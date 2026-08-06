//! Verifier logic and the text-only-answer branch.
//!
//! `verify_answer` consults the configured `Critic`; on rejection it injects
//! feedback into the context. `verify_final_text` wraps the text-only branch
//! of one iteration: verify, then on pass return `IterOutcome::FinalText` so
//! the driver can call `phases::finalize::emit_final_text`.

use super::{IterOutcome, LoopState, ThinkOutput, with_reasoning_content};
use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::Result;
use crate::llm::types::Message;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

/// Verify a final answer with the configured Critic.
///
/// Returns `true` if the answer passes verification (or verification is
/// disabled). Returns `false` if the answer fails and feedback has been
/// injected into the context for self-correction.
pub(crate) async fn verify_answer(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    answer: &str,
    retry_count: usize,
) -> bool {
    // Skip if verifier is disabled or no critic is configured
    if !snap.config.verifier_enabled {
        return true;
    }
    let Some(ref critic) = snap.critic else {
        return true;
    };
    // Don't retry beyond max (but always allow the first check)
    if retry_count > 0 && retry_count >= snap.config.verifier_max_retries {
        tracing::info!(
            retries = retry_count,
            "Verifier max retries reached, accepting answer"
        );
        return true;
    }

    let task_description = {
        let ctx = context.lock().await;
        // Use the last user message as the task description
        ctx.messages()
            .iter()
            .rev()
            .find(|m| m.role == echo_core::llm::types::Role::User)
            .map(|m| {
                m.content
                    .as_text()
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    };

    match critic.critique(&task_description, answer, "").await {
        Ok(critique) => {
            if critique.passed || critique.score >= snap.config.verifier_min_score {
                tracing::debug!(score = critique.score, "Verifier passed, accepting answer");
                true
            } else {
                let feedback = format!(
                    "[Verifier feedback] Score: {}/10 (min: {}). {}\nSuggestions: {}",
                    critique.score,
                    snap.config.verifier_min_score,
                    critique.feedback,
                    if critique.suggestions.is_empty() {
                        "N/A".to_string()
                    } else {
                        critique.suggestions.join(", ")
                    },
                );
                tracing::info!(
                    score = critique.score,
                    retry = retry_count + 1,
                    max = snap.config.verifier_max_retries,
                    "Verifier rejected answer, injecting feedback for self-correction"
                );
                super::super::context::push_runtime_context_note(
                    context,
                    "VerifierFeedback",
                    &feedback,
                )
                .await;
                false
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "Verifier critique failed, accepting answer");
            true // Fail-open: accept answer if critique itself errors
        }
    }
}

/// Text-only branch of one iteration. Verifies the buffered LLM content; on
/// failure pushes the assistant attempt into context, increments
/// `state.verifier_retry_count`, and returns [`IterOutcome::Continue`] so
/// the loop drives a self-correction iteration. On success returns
/// [`IterOutcome::FinalText`] with the answer — the driver calls
/// `emit_final_text` to handle audit, checkpoint, transcript projection,
/// `finalize_run`, the `FinalAnswer` event, and the `Stop` hook
/// continuation.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn verify_final_text(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    _tx: &mpsc::Sender<Result<AgentEvent>>,
    state: &mut LoopState,
    _iteration: usize,
    think: ThinkOutput,
    _label: &str,
) -> Result<IterOutcome> {
    let content_buffer = think.content_buffer;
    let reasoning_content = think.reasoning_buffer;
    if !verify_answer(snap, context, &content_buffer, state.verifier_retry_count).await {
        // Push the LLM's answer to context so it can see its own attempt
        context.lock().await.push(with_reasoning_content(
            Message::assistant(content_buffer),
            reasoning_content,
        ));
        state.verifier_retry_count += 1;
        return Ok(IterOutcome::Continue);
    }

    Ok(IterOutcome::FinalText {
        answer: content_buffer,
        reasoning_content,
    })
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ReactAgent;
    use crate::agent::config::AgentConfig;
    use echo_core::agent::StaticCritic;

    /// `verifier_enabled = false` (the default) accepts every answer without
    /// touching the critic or the context.
    #[tokio::test]
    async fn verify_answer_disabled_returns_true() {
        let config = AgentConfig::new("test-model", "agent", "sys");
        let agent = ReactAgent::new(config);
        let snap = AgentRunSnapshot::from_agent(&agent);

        assert!(!snap.config.verifier_enabled);
        let accepted = verify_answer(&snap, &agent.memory.context, "any answer", 0).await;
        assert!(
            accepted,
            "verifier disabled should fail-open without inspecting the critic",
        );
    }

    /// `verifier_enabled = true` but no critic configured accepts every
    /// answer.
    #[tokio::test]
    async fn verify_answer_no_critic_returns_true() {
        let config = AgentConfig::new("test-model", "agent", "sys").verifier_enabled(true);
        let agent = ReactAgent::new(config);
        let snap = AgentRunSnapshot::from_agent(&agent);

        assert!(snap.critic.is_none());
        let accepted = verify_answer(&snap, &agent.memory.context, "answer", 0).await;
        assert!(accepted, "no critic should fail-open with true");
    }

    /// On the first failed critique, feedback is injected into context and
    /// the answer is rejected. The retry counter starts at 0 so the
    /// "max retries reached" short-circuit must NOT fire.
    #[tokio::test]
    async fn verify_answer_low_score_pushes_feedback_and_rejects() {
        let config = AgentConfig::new("test-model", "agent", "sys")
            .verifier_enabled(true)
            .verifier_min_score(7.0);
        let mut agent = ReactAgent::new(config);
        agent.set_critic(Arc::new(StaticCritic::always_fail()));
        let snap = AgentRunSnapshot::from_agent(&agent);

        let before = agent.memory.context.lock().await.messages().len();
        let accepted = verify_answer(&snap, &agent.memory.context, "bad answer", 0).await;
        let after = agent.memory.context.lock().await.messages().len();

        assert!(!accepted, "low-score critique should reject the answer");
        assert_eq!(
            after,
            before + 1,
            "rejection should inject one feedback message into context",
        );
        let last = agent.memory.context.lock().await.messages().last().cloned();
        let text = last
            .as_ref()
            .and_then(|m| m.content.as_text().map(|s| s.to_string()))
            .unwrap_or_default();
        assert!(
            text.starts_with("[runtime_context:VerifierFeedback]")
                && text.contains("[Verifier feedback]"),
            "injected feedback should be runtime context, got: {text:?}",
        );
    }

    /// Once `retry_count` reaches `verifier_max_retries`, even a failing
    /// critic causes acceptance (avoids infinite self-correction loops).
    #[tokio::test]
    async fn verify_answer_max_retries_short_circuits_to_true() {
        let config = AgentConfig::new("test-model", "agent", "sys")
            .verifier_enabled(true)
            .verifier_max_retries(2);
        let mut agent = ReactAgent::new(config);
        agent.set_critic(Arc::new(StaticCritic::always_fail()));
        let snap = AgentRunSnapshot::from_agent(&agent);

        let accepted = verify_answer(&snap, &agent.memory.context, "answer", 2).await;
        assert!(
            accepted,
            "retry_count >= max_retries should accept without consulting the critic",
        );
    }
}
