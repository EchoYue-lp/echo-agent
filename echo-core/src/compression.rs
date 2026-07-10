//! Context compression — trait and input/output types
//!
//! Implementations live in `echo_state::compression::compressor`.

use crate::error::Result;
use crate::llm::types::Message;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

/// A replaceable, marker-tagged message projected into model context.
#[derive(Debug, Clone)]
pub struct ContextProjection {
    /// Stable content marker used to identify the projection across refreshes.
    pub marker: String,
    /// Current projected message, or `None` to remove a stale projection.
    pub message: Option<Message>,
}

/// Generic run metadata available to a pre-model context projector.
#[derive(Debug, Clone)]
pub struct ProjectionContext {
    pub iteration: usize,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
}

/// Produces replaceable context immediately before model input preparation.
pub trait PreModelContextProjector: Send + Sync {
    fn project<'a>(
        &'a self,
        context: &'a ProjectionContext,
    ) -> BoxFuture<'a, Result<Vec<ContextProjection>>>;
}

/// Compression pipeline input
pub struct CompressionInput {
    /// Messages to be compressed
    pub messages: Vec<Message>,
    /// Token limit, triggers compression when exceeded
    pub token_limit: usize,
    /// Current user query — used to protect active task context from eviction
    pub current_query: Option<String>,
    /// Focus instructions — user-provided guidance on what to prioritize in summaries
    pub focus_instructions: Option<String>,
}

/// Compression pipeline output
pub struct CompressionOutput {
    /// Final list of messages to keep and send to the LLM
    pub messages: Vec<Message>,
    /// Messages evicted in this compression pass
    pub evicted: Vec<Message>,
    /// Compression checkpoint for audit, replay, and recovery
    pub checkpoint: Option<CompressionCheckpoint>,
}

/// Records a fix applied to maintain valid tool_call ↔ tool_result pairing after compression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPairFix {
    pub tool_call_id: String,
    pub fix_type: ToolPairFixType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolPairFixType {
    /// An orphaned tool_result (no matching assistant tool_calls) was removed.
    OrphanedResultRemoved,
    /// All tool_calls in an assistant message were cleared (no matching results).
    DanglingCallCleared,
    /// A placeholder result was inserted for a missing tool_result.
    PlaceholderResultInserted,
}

/// A first-class state object capturing the complete result of a compression pass.
///
/// This is the audit trail for context compression — it records what was done,
/// what was removed, what was fixed, and what was promoted to long-term memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionCheckpoint {
    /// Unique identifier for this checkpoint (UUID v4).
    pub checkpoint_id: String,
    /// Name of the compression strategy used (e.g. "SlidingWindow", "Summary").
    pub strategy: String,
    /// Index range [first, last] of messages covered by this compression pass.
    pub covered_range: Option<(usize, usize)>,
    /// LLM-generated summary, if this strategy produces one.
    pub summary: Option<String>,
    /// Number of messages retained after compression.
    pub retained_count: usize,
    /// Number of messages evicted during compression.
    pub evicted_count: usize,
    /// Number of protected messages excluded from compression.
    pub protected_count: usize,
    /// Tool-call pairing fixes applied after compression.
    pub tool_pair_fixes: Vec<ToolPairFix>,
    /// Number of evicted messages sent to the MemoryPromoter for fact extraction.
    /// Note: this counts evicted messages, not actual extracted facts (promoter runs async).
    pub memory_promotion_count: usize,
    /// Estimated tokens before compression.
    pub token_before: usize,
    /// Estimated tokens after compression.
    pub token_after: usize,
    /// Wall-clock duration of the compression pass in milliseconds.
    pub compression_duration_ms: u64,
    /// Timestamp when this checkpoint was created.
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub created_at: DateTime<Utc>,
    /// User-provided focus instructions for this compression, if any.
    pub focus_instructions: Option<String>,
    /// Compression levels applied (relevant for AdaptiveCompressor).
    pub levels_applied: Vec<String>,
}

impl CompressionCheckpoint {
    /// Create a new checkpoint with a random UUID and the current timestamp.
    pub fn new(strategy: impl Into<String>) -> Self {
        Self {
            checkpoint_id: uuid::Uuid::new_v4().to_string(),
            strategy: strategy.into(),
            covered_range: None,
            summary: None,
            retained_count: 0,
            evicted_count: 0,
            protected_count: 0,
            tool_pair_fixes: Vec::new(),
            memory_promotion_count: 0,
            token_before: 0,
            token_after: 0,
            compression_duration_ms: 0,
            created_at: Utc::now(),
            focus_instructions: None,
            levels_applied: Vec::new(),
        }
    }

    /// Builder-style: set the covered message range.
    pub fn with_covered_range(mut self, first: usize, last: usize) -> Self {
        self.covered_range = Some((first, last));
        self
    }

    /// Builder-style: set the summary text.
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Builder-style: set token counts.
    pub fn with_tokens(mut self, before: usize, after: usize) -> Self {
        self.token_before = before;
        self.token_after = after;
        self
    }

    /// Builder-style: set message counts.
    pub fn with_counts(mut self, retained: usize, evicted: usize) -> Self {
        self.retained_count = retained;
        self.evicted_count = evicted;
        self
    }

    /// Builder-style: set protected message count.
    pub fn with_protected_count(mut self, count: usize) -> Self {
        self.protected_count = count;
        self
    }

    /// Builder-style: set memory promotion count.
    pub fn with_memory_promotion_count(mut self, count: usize) -> Self {
        self.memory_promotion_count = count;
        self
    }

    /// Builder-style: set tool pair fixes.
    pub fn with_tool_fixes(mut self, fixes: Vec<ToolPairFix>) -> Self {
        self.tool_pair_fixes = fixes;
        self
    }

    /// Builder-style: set the compression duration.
    pub fn with_duration_ms(mut self, ms: u64) -> Self {
        self.compression_duration_ms = ms;
        self
    }

    /// Builder-style: set focus instructions.
    pub fn with_focus(mut self, focus: Option<String>) -> Self {
        self.focus_instructions = focus;
        self
    }

    /// Builder-style: set levels applied (for AdaptiveCompressor).
    pub fn with_levels(mut self, levels: Vec<String>) -> Self {
        self.levels_applied = levels;
        self
    }
}

/// Structured summary produced by LLM-based compressors.
///
/// Unlike free-text summaries, each field tracks a specific aspect of the
/// conversation. This enables:
/// - **Field-level merge** for incremental compression (no summary drift)
/// - **Verification** — check each field independently
/// - **Programmatic access** — extract file paths, pending tasks, etc.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StructuredSummary {
    /// User's primary goal and intent
    pub goal: String,
    /// The specific task currently being worked on
    pub current_task: String,
    /// Actions that have been completed
    pub completed_actions: Vec<String>,
    /// Tasks that still need to be done
    pub pending_tasks: Vec<String>,
    /// Decisions made during the conversation
    pub decisions: Vec<String>,
    /// File paths referenced or modified
    pub files_touched: Vec<String>,
    /// Errors encountered and their resolutions
    pub errors: Vec<String>,
    /// Summary of key findings from tool outputs
    pub tool_outputs_summary: String,
    /// User preferences or constraints discovered
    pub user_preferences: Vec<String>,
    /// Suggested next step
    pub next_step: String,
}

impl StructuredSummary {
    /// Parse from a JSON string (LLM response).
    pub fn from_json(json: &str) -> std::result::Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Attempt to extract and parse JSON from an LLM response that may contain
    /// markdown fences or extra text.
    pub fn from_llm_response(response: &str) -> Option<Self> {
        // Try direct parse first
        if let Ok(s) = Self::from_json(response) {
            return Some(s);
        }
        // Try extracting from markdown code fences
        if let Some(json) = extract_json_from_text(response) {
            if let Ok(s) = Self::from_json(&json) {
                return Some(s);
            }
        }
        None
    }

    /// Serialize to a compact JSON string for storage.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Format as a system message content: `[对话历史摘要]\n{json}`
    pub fn to_system_message(&self) -> String {
        format!("[对话历史摘要]\n{}", self.to_json())
    }

    /// Merge newer summary information into this one (field-level).
    ///
    /// Used by `IncrementalSummaryCompressor` to update the cached summary
    /// without losing information from earlier compression passes.
    pub fn merge_with(&mut self, newer: &StructuredSummary) {
        // goal: track the most recent goal (latest wins, no accumulation noise)
        if !newer.goal.is_empty() && newer.goal != self.goal {
            self.goal = newer.goal.clone();
        }

        // current_task: latest value wins
        if !newer.current_task.is_empty() {
            self.current_task = newer.current_task.clone();
        }

        // completed_actions: append new, deduplicate
        for action in &newer.completed_actions {
            if !self.completed_actions.contains(action) {
                self.completed_actions.push(action.clone());
            }
        }

        // pending_tasks: remove completed ones, add new ones (but not those already completed)
        let completed_set: std::collections::HashSet<&String> =
            newer.completed_actions.iter().collect();
        // Also consider our own completed_actions when deciding what to retain
        let all_completed: std::collections::HashSet<&String> =
            self.completed_actions.iter().collect();
        self.pending_tasks.retain(|t| !completed_set.contains(t));
        for task in &newer.pending_tasks {
            if !self.pending_tasks.contains(task)
                && !completed_set.contains(task)
                && !all_completed.contains(task)
            {
                self.pending_tasks.push(task.clone());
            }
        }

        // decisions: append new, deduplicate
        for decision in &newer.decisions {
            if !self.decisions.contains(decision) {
                self.decisions.push(decision.clone());
            }
        }

        // files_touched: normalize paths, merge and deduplicate
        for file in &newer.files_touched {
            let normalized = normalize_path(file);
            if !self.files_touched.contains(&normalized) {
                self.files_touched.push(normalized);
            }
        }

        // errors: append new, deduplicate
        for error in &newer.errors {
            if !self.errors.contains(error) {
                self.errors.push(error.clone());
            }
        }

        // tool_outputs_summary: latest value wins
        if !newer.tool_outputs_summary.is_empty() {
            self.tool_outputs_summary = newer.tool_outputs_summary.clone();
        }

        // user_preferences: append new, deduplicate
        for pref in &newer.user_preferences {
            if !self.user_preferences.contains(pref) {
                self.user_preferences.push(pref.clone());
            }
        }

        // next_step: latest value wins
        if !newer.next_step.is_empty() {
            self.next_step = newer.next_step.clone();
        }
    }
}

/// Tracks canonical context sources that should survive compression.
///
/// When compression evicts messages, critical context (system prompt, project rules,
/// skill injections) may be lost. `CanonicalContext` stores the authoritative sources
/// so they can be re-injected after compression.
#[derive(Debug, Clone, Default)]
pub struct CanonicalContext {
    /// The base system prompt (without skill/rule extensions).
    pub system_prompt: Option<String>,
    /// Project-level rules (AGENT.md / RULES.md content).
    pub project_rules: Option<String>,
    /// System prompt injections from activated skills.
    pub skill_injections: Vec<String>,
    /// Names of currently active skills.
    pub active_skill_names: Vec<String>,
}

impl CanonicalContext {
    /// Returns true if any canonical source is configured.
    pub fn has_any(&self) -> bool {
        self.system_prompt.is_some()
            || self.project_rules.is_some()
            || !self.skill_injections.is_empty()
    }

    /// Build supplemental re-injection messages inserted after compression.
    ///
    /// The base system prompt is restored directly by `ContextManager`; this
    /// method returns only supplemental canonical context so the prompt is not
    /// represented twice.
    /// Returns `None` if there's nothing to re-inject.
    pub fn to_reinjection_messages(&self) -> Option<Vec<String>> {
        if !self.has_any() {
            return None;
        }

        let mut msgs: Vec<String> = Vec::new();

        // Re-inject project rules as a separate system message
        if let Some(ref rules) = self.project_rules {
            let truncated: String = rules.chars().take(2000).collect();
            msgs.push(format!(
                "[Canonical context — project rules restored]:\n{}",
                truncated
            ));
        }

        // List active skills for LLM awareness
        if !self.active_skill_names.is_empty() {
            msgs.push(format!(
                "[Canonical context — active skills]: {}",
                self.active_skill_names.join(", ")
            ));
        }

        Some(msgs)
    }
}

/// Normalize a file path for deduplication: strip leading `./`, trim whitespace.
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if let Some(stripped) = trimmed.strip_prefix("./") {
        stripped.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Extract JSON content from text that may contain markdown fences or extra text.
fn extract_json_from_text(text: &str) -> Option<String> {
    // Try ```json ... ``` code fence
    if let Some(start) = text.find("```json") {
        let after_fence = text.get(start.saturating_add(7)..)?;
        if let Some(end) = after_fence.find("```") {
            return after_fence.get(..end).map(|value| value.trim().to_string());
        }
    }
    // Try ``` ... ``` code fence
    if let Some(start) = text.find("```") {
        let after_fence = text.get(start.saturating_add(3)..)?;
        if let Some(end) = after_fence.find("```") {
            let candidate = after_fence.get(..end)?.trim().to_string();
            if candidate.starts_with('{') {
                return Some(candidate);
            }
        }
    }
    // Try finding { ... } block
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            let candidate = text.get(start..=end)?.to_string();
            if candidate.chars().count() >= 10 {
                return Some(candidate);
            }
        }
    }
    None
}

/// Unified interface for all compression strategies (async, supports `dyn` trait object)
pub trait ContextCompressor: Send + Sync {
    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>>;

    /// Human-readable name of this compressor, used for metrics tracking.
    /// Override in implementations for a descriptive name.
    fn name(&self) -> &'static str {
        "custom"
    }
}

/// Allows `Box<dyn ContextCompressor>` to be passed directly to any function accepting
/// `impl ContextCompressor`, without introducing an extra wrapper enum.
impl ContextCompressor for Box<dyn ContextCompressor> {
    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        (**self).compress(input)
    }

    fn name(&self) -> &'static str {
        (**self).name()
    }
}
