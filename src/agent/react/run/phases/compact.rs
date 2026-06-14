//! Per-iteration compaction: fire `PreCompact`, save runtime checkpoint,
//! run `ContextManager::prepare`, fire `PostCompact`, return prepared messages.

use super::super::stream_macros::{try_send_or, yield_event_or};
use super::CompactOutcome;
use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::Result;
use crate::llm::types::Message;
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
    _iteration: usize,
) -> Result<CompactOutcome> {
    snap.fire_hook(crate::skills::hooks::HookEvent::PreCompact, Some("auto"))
        .await;
    // Save checkpoint before compression (preserves full context)
    snap.save_runtime_checkpoint(context, None).await;
    let prepare_result = try_send_or!(
        tx,
        context.lock().await.prepare(None).await,
        CompactOutcome::Abandoned
    );

    if let Some(ref stats) = prepare_result.compressed {
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
            context
                .lock()
                .await
                .push(Message::system(format!("[Hook:PostCompact] {}", c)));
        }
        for m in &r.messages {
            context.lock().await.push(Message::system(m.clone()));
        }
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
    use crate::llm::types::{Message, Role};

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
            CompactOutcome::Abandoned => panic!("default config should not abandon"),
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
        assert!(matches!(outcome, CompactOutcome::Continue(_)));
    }
}
