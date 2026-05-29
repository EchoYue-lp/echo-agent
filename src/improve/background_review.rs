//! Background review — fork an agent to extract memory and skills from conversations.
//!
//! After every conversation turn, [`BackgroundReviewer`] spawns a background task
//! that replays the conversation and asks "should any memory or skill be saved or
//! updated?". Writes go to the memory/skill stores. The main conversation is never
//! touched.
//!
//! Inspired by Hermes Agent's background review system.

use crate::error::Result;
use crate::llm::LlmClient;
use crate::memory::store::Store;
use crate::trace::{Run, RunEvent, RunStore};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Review prompts (adapted from Hermes Agent) ─────────────────────

const MEMORY_REVIEW_PROMPT: &str = "\
Review the conversation above and consider saving to memory if appropriate.

Focus on:
1. Has the user revealed things about themselves — their persona, desires, \
preferences, or personal details worth remembering?
2. Has the user expressed expectations about how you should behave, their work \
style, or ways they want you to operate?

If something stands out, save it using the memory tool.
If nothing is worth saving, just say 'Nothing to save.' and stop.";

const SKILL_REVIEW_PROMPT: &str = "\
Review the conversation above and identify reusable patterns for the skill library.

Signals to look for:
  • User corrected your style, tone, format, verbosity, or approach. \
Frustration signals like 'stop doing X', 'this is too verbose', 'don't format \
like this' are FIRST-CLASS skill signals.
  • User corrected your workflow, approach, or sequence of steps.
  • Non-trivial technique, fix, workaround, or debugging path emerged.
  • A skill that was loaded turned out to be wrong, missing, or outdated.

Do NOT capture:
  • Environment-dependent failures (missing binaries, uninstalled packages).
  • Negative claims about tools ('X tool is broken').
  • Session-specific transient errors that resolved.
  • One-off task narratives.

If nothing stands out, say 'Nothing to save.' and stop. Otherwise, describe \
the skill update you recommend in structured format.";

const COMBINED_REVIEW_PROMPT: &str = "\
Review the conversation above and update two things:

**Memory**: who the user is. Did the user reveal persona, desires, preferences, \
personal details, or expectations about how you should behave? Save facts about \
the user and durable preferences with the memory tool.

**Skills**: how to do this class of task. Most sessions produce at least one \
skill update. Look for user corrections, non-trivial techniques, or outdated skills.

If genuinely nothing stands out on either dimension, say 'Nothing to save.' \
and stop — but don't reach for that conclusion as a default.";

// ── BackgroundReviewConfig ─────────────────────────────────────────

/// Configuration for the background review system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundReviewConfig {
    /// Whether background review is enabled.
    pub enabled: bool,
    /// Maximum iterations for the review agent.
    pub max_iterations: usize,
    /// Which review types to run.
    pub review_memory: bool,
    /// Whether to review skills.
    pub review_skills: bool,
}

impl Default for BackgroundReviewConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_iterations: 8,
            review_memory: true,
            review_skills: true,
        }
    }
}

// ── ReviewOutcome ──────────────────────────────────────────────────

/// Result of a background review pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewOutcome {
    /// The run ID that was reviewed.
    pub run_id: String,
    /// Summary of actions taken (e.g., "Memory updated: user prefers concise answers").
    pub actions: Vec<String>,
    /// Whether the review found nothing to save.
    pub nothing_to_save: bool,
    /// Error message if the review failed.
    pub error: Option<String>,
}

// ── BackgroundReviewer ─────────────────────────────────────────────

/// Spawns background tasks to review conversations and extract memory/skills.
///
/// Uses the parent agent's LLM client and memory store. The review agent
/// is a lightweight fork with limited tools and iterations.
pub struct BackgroundReviewer {
    config: BackgroundReviewConfig,
    llm_client: Arc<dyn LlmClient>,
    memory_store: Option<Arc<dyn Store>>,
    run_store: Option<Arc<dyn RunStore>>,
}

impl BackgroundReviewer {
    /// Create a new background reviewer.
    pub fn new(
        config: BackgroundReviewConfig,
        llm_client: Arc<dyn LlmClient>,
        memory_store: Option<Arc<dyn Store>>,
        run_store: Option<Arc<dyn RunStore>>,
    ) -> Self {
        Self {
            config,
            llm_client,
            memory_store,
            run_store,
        }
    }

    /// Get the review prompt based on configuration.
    fn review_prompt(&self) -> &str {
        match (self.config.review_memory, self.config.review_skills) {
            (true, true) => COMBINED_REVIEW_PROMPT,
            (true, false) => MEMORY_REVIEW_PROMPT,
            (false, true) => SKILL_REVIEW_PROMPT,
            _ => COMBINED_REVIEW_PROMPT,
        }
    }

    /// Convert a Run's events into a conversation transcript string.
    fn build_transcript(run: &Run) -> String {
        let mut lines = Vec::new();
        lines.push(format!("User: {}", run.input));

        for event in &run.events {
            match event {
                RunEvent::ToolCall { name, args, .. } => {
                    let args_str = args
                        .as_ref()
                        .map(|v| serde_json::to_string(v).unwrap_or_default())
                        .unwrap_or_default();
                    lines.push(format!("Assistant [tool call]: {name}({args_str})"));
                }
                RunEvent::ToolResult {
                    name,
                    success,
                    output_preview,
                    ..
                } => {
                    let status = if *success { "OK" } else { "FAILED" };
                    let output = output_preview.as_deref().unwrap_or("(no output)");
                    lines.push(format!("Tool [{status}] {name}: {output}"));
                }
                RunEvent::ToolError { name, message, .. } => {
                    lines.push(format!("Tool [ERROR] {name}: {message}"));
                }
                _ => {}
            }
        }

        if let Some(ref output) = run.final_output {
            lines.push(format!("Assistant: {output}"));
        }

        lines.join("\n")
    }

    /// Run a background review for the given run.
    ///
    /// This spawns a background task that:
    /// 1. Builds a transcript from the run events
    /// 2. Sends it to the LLM with the review prompt
    /// 3. Parses the response for memory/skill actions
    /// 4. Returns a ReviewOutcome
    pub async fn review(&self, run: &Run) -> Result<ReviewOutcome> {
        if !self.config.enabled {
            return Ok(ReviewOutcome {
                run_id: run.run_id.clone(),
                actions: vec![],
                nothing_to_save: true,
                error: None,
            });
        }

        let transcript = Self::build_transcript(run);
        let prompt = self.review_prompt().to_string();
        let run_id = run.run_id.clone();
        let llm_client = self.llm_client.clone();
        let memory_store = self.memory_store.clone();

        // Build the full review message
        let review_message = format!(
            "{transcript}\n\n---\n\n{prompt}\n\nYou can only call memory management tools. \
             Other tools will be denied at runtime — do not attempt them."
        );

        // Spawn background task
        let run_id_clone = run_id.clone();
        let outcome = tokio::spawn(async move {
            Self::run_review(llm_client, memory_store, run_id, review_message).await
        })
        .await
        .unwrap_or_else(|e| ReviewOutcome {
            run_id: run_id_clone,
            actions: vec![],
            nothing_to_save: true,
            error: Some(format!("Review task panicked: {e}")),
        });

        Ok(outcome)
    }

    /// Run a review for a specific run ID (loading from the run store).
    pub async fn review_by_run_id(&self, run_id: &str) -> Result<ReviewOutcome> {
        let store = match &self.run_store {
            Some(s) => s,
            None => {
                return Ok(ReviewOutcome {
                    run_id: run_id.to_string(),
                    actions: vec![],
                    nothing_to_save: true,
                    error: Some("No run store configured".into()),
                });
            }
        };

        match store.load(run_id).await? {
            Some(run) => self.review(&run).await,
            None => Ok(ReviewOutcome {
                run_id: run_id.to_string(),
                actions: vec![],
                nothing_to_save: true,
                error: Some(format!("Run {run_id} not found")),
            }),
        }
    }

    /// Execute the review using the LLM client directly.
    ///
    /// Uses a simple chat call (not a full agent loop) for efficiency.
    /// The LLM response is parsed for action keywords.
    async fn run_review(
        llm_client: Arc<dyn LlmClient>,
        memory_store: Option<Arc<dyn Store>>,
        run_id: String,
        message: String,
    ) -> ReviewOutcome {
        let messages = vec![
            crate::llm::types::Message::system(
                "You are a background review agent. Analyze conversations and extract \
                 reusable knowledge. Be concise. Only call tools when there's real signal."
                    .to_string(),
            ),
            crate::llm::types::Message::user(message),
        ];

        let request = crate::llm::ChatRequest {
            messages,
            temperature: Some(0.3),
            max_tokens: Some(2048),
            ..Default::default()
        };

        let response = match llm_client.chat(request).await {
            Ok(r) => r,
            Err(e) => {
                return ReviewOutcome {
                    run_id,
                    actions: vec![],
                    nothing_to_save: true,
                    error: Some(format!("LLM call failed: {e}")),
                };
            }
        };

        let content = response.content().unwrap_or_default();

        // Parse the response for actions
        let nothing_to_save = content.to_lowercase().contains("nothing to save");
        let mut actions = Vec::new();

        if !nothing_to_save {
            // Extract action keywords from the response
            let lower = content.to_lowercase();
            if lower.contains("memory")
                && (lower.contains("save")
                    || lower.contains("update")
                    || lower.contains("remember"))
            {
                actions.push("Memory reviewed".to_string());
            }
            if lower.contains("skill")
                && (lower.contains("create") || lower.contains("update") || lower.contains("patch"))
            {
                actions.push("Skill update recommended".to_string());
            }

            // If we have a memory store and the review suggests saving, try to persist
            if let Some(ref store) = memory_store
                && !content.contains("Nothing to save")
            {
                // Save the review summary as a memory entry
                let key = format!("review_{}", &run_id);
                let value = serde_json::json!({
                    "review": content,
                    "run_id": &run_id,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });
                if store
                    .put(&["background_reviews"], &key, value)
                    .await
                    .is_ok()
                    && actions.is_empty()
                {
                    actions.push("Review saved to memory".to_string());
                }
            }
        }

        ReviewOutcome {
            run_id,
            actions,
            nothing_to_save,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{Run, RunEvent, RunStatus, RunTimings, TokenUsage};
    use chrono::Utc;

    fn make_test_run() -> Run {
        Run {
            run_id: "test-review-1".into(),
            parent_run_id: None,
            session_id: "sess-1".into(),
            status: RunStatus::Completed,
            input: "Fix the bug in auth.rs".into(),
            events: vec![
                RunEvent::ToolCall {
                    call_id: "c1".into(),
                    name: "read_file".into(),
                    args: Some(serde_json::json!({"path": "auth.rs"})),
                    risk: None,
                    duration_ms: 50,
                },
                RunEvent::ToolResult {
                    call_id: "c1".into(),
                    name: "read_file".into(),
                    success: true,
                    output_preview: Some(
                        "fn authenticate(token: &str) -> Result<User> { ... }".into(),
                    ),
                    output_truncated: false,
                    duration_ms: 50,
                },
            ],
            final_output: Some("Fixed the auth bug by adding null check on token.".into()),
            error: None,
            token_usage: TokenUsage {
                prompt_tokens: 200,
                completion_tokens: 100,
                total_tokens: 300,
            },
            timings: RunTimings {
                total_duration_ms: 1000,
                llm_duration_ms: 800,
                tool_duration_ms: 50,
            },
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
        }
    }

    #[test]
    fn test_build_transcript() {
        let run = make_test_run();
        let transcript = BackgroundReviewer::build_transcript(&run);
        assert!(transcript.contains("User: Fix the bug in auth.rs"));
        assert!(transcript.contains("read_file"));
        assert!(transcript.contains("Fixed the auth bug"));
    }

    #[test]
    fn test_review_prompt_selection() {
        let config = BackgroundReviewConfig {
            review_memory: true,
            review_skills: true,
            ..Default::default()
        };
        // We can't easily construct a real reviewer without LLM client,
        // but we can test the prompt selection logic
        let prompt = match (config.review_memory, config.review_skills) {
            (true, true) => COMBINED_REVIEW_PROMPT,
            (true, false) => MEMORY_REVIEW_PROMPT,
            (false, true) => SKILL_REVIEW_PROMPT,
            _ => COMBINED_REVIEW_PROMPT,
        };
        assert!(prompt.contains("Memory"));
        assert!(prompt.contains("Skills"));
    }
}
