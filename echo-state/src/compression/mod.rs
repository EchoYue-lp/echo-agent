//! Context compression
//!
//! Maintains conversation history and automatically compresses when tokens exceed the limit, managed by [`ContextManager`].
//!
//! Built-in compression strategies (all implement the [`ContextCompressor`] trait):
//! - [`compressor::SlidingWindowCompressor`]: Sliding window, discards the oldest N messages
//! - [`compressor::SummaryCompressor`]: LLM summarization, compresses old messages into a system summary message (stage4 P4.3 default)

pub mod compressor;
pub mod horizon;
pub mod invariants;
pub mod levels;
pub mod verifier;

// Re-export from echo_core for backward compatibility
pub use echo_core::compression::{
    CanonicalContext, CompressionCheckpoint, CompressionInput, CompressionOutput,
    ContextCompressor, ContextProjection, PreModelContextProjector, ProjectionContext,
    StructuredSummary, ToolPairFix, ToolPairFixType,
};

use crate::compression::compressor::SlidingWindowCompressor;
use echo_core::budget::TokenBudget;
use echo_core::error::Result;
use echo_core::llm::types::{ContentPart, Message, MessageContent, Role};
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use std::sync::Arc;

const PROJECTION_ENVELOPE_PREFIX: &str = "<echo-agent-context-projection-v1>";

/// Return whether a message carries the framework-owned context projection envelope.
///
/// Consumers should use this predicate instead of matching the private envelope
/// representation directly.
pub fn is_context_projection_message(message: &Message) -> bool {
    projection_envelope_text(message).is_some()
}

fn projection_envelope_text(message: &Message) -> Option<&str> {
    match &message.content {
        MessageContent::Text(content) => content
            .starts_with(PROJECTION_ENVELOPE_PREFIX)
            .then_some(content.as_str()),
        MessageContent::Parts(parts) => parts.first().and_then(|part| match part {
            ContentPart::Text { text } if text.starts_with(PROJECTION_ENVELOPE_PREFIX) => {
                Some(text.as_str())
            }
            _ => None,
        }),
        MessageContent::Empty => None,
    }
}

/// Callback trait for promoting evicted messages to long-term memory.
///
/// When compression evicts messages from the context, this trait allows
/// extracting key facts and writing them to a memory Store for later recall.
/// This is the "L3 Memory Promotion" mechanism.
pub trait MemoryPromoter: Send + Sync {
    /// Process evicted messages and promote key facts to memory.
    fn promote(&self, evicted: &[Message]) -> futures::future::BoxFuture<'_, ()>;
}

/// Blanket implementation for `Arc<dyn MemoryPromoter>`.
impl MemoryPromoter for Arc<dyn MemoryPromoter> {
    fn promote(&self, evicted: &[Message]) -> futures::future::BoxFuture<'_, ()> {
        (**self).promote(evicted)
    }
}

/// Metadata needed to restore protected messages near their original positions.
struct ProtectedMessage {
    message: Message,
    /// Number of compressible messages that originally appeared after this message.
    compressible_after: usize,
    /// Number of protected messages that originally appeared after this message.
    protected_after: usize,
}

/// Compression statistics returned by `force_compress()`
pub struct ForceCompressStats {
    /// Total message count before compression
    pub before_count: usize,
    /// Total message count after compression
    pub after_count: usize,
    /// Number of messages evicted
    pub evicted: usize,
    /// Estimated token count before compression
    pub before_tokens: usize,
    /// Estimated token count after compression
    pub after_tokens: usize,
}

/// Result of `ContextManager::prepare()` — includes the prepared messages and
/// optional compression stats if auto-compression was triggered.
pub struct PrepareResult {
    /// The prepared message list to send to the LLM.
    pub messages: Vec<Message>,
    /// Compression statistics, populated only when auto-compression occurred.
    pub compressed: Option<ForceCompressStats>,
    /// Compression checkpoint for audit, replay, and recovery.
    pub checkpoint: Option<CompressionCheckpoint>,
    /// Summary verification results (only when a summary was produced).
    pub verification: Option<verifier::SummaryVerification>,
}

/// Token breakdown by message role.
///
/// Used by `/context` command to show where context window budget is spent.
#[derive(Debug, Clone)]
pub struct TokenBreakdown {
    pub system: usize,
    pub user: usize,
    pub assistant: usize,
    pub tool: usize,
    pub summary: usize,
    pub memory: usize,
    pub total: usize,
    pub max_context: Option<usize>,
    pub token_limit: usize,
    pub compression_count: u64,
    pub compression_ratio: f64,
}

impl TokenBreakdown {
    /// Format as a human-readable progress bar display.
    pub fn format_bar(&self) -> String {
        let max = self.max_context.unwrap_or(self.total.max(1));
        let pct = |n: usize| -> f64 { if max == 0 { 0.0 } else { n as f64 / max as f64 } };

        let bar = |label: &str, n: usize| -> String {
            let frac = pct(n);
            let filled = (frac * 20.0) as usize;
            let bar_str: String = (0..20)
                .map(|i| if i < filled { '█' } else { '░' })
                .collect();
            format!(
                "  {:<12} {} {:>6} ({:>4.1}%)",
                label,
                bar_str,
                fmt_tokens(n),
                frac * 100.0
            )
        };

        let mut s = format!(
            "  Messages: {}  Tokens: {} / {} ({:.1}%)\n",
            self.system + self.user + self.assistant + self.tool + self.summary + self.memory,
            fmt_tokens(self.total),
            fmt_tokens(max),
            pct(self.total) * 100.0,
        );
        s.push_str(&bar("System:", self.system));
        s.push('\n');
        s.push_str(&bar("User:", self.user));
        s.push('\n');
        s.push_str(&bar("Assistant:", self.assistant));
        s.push('\n');
        s.push_str(&bar("Tool:", self.tool));
        s.push('\n');
        s.push_str(&bar("Summary:", self.summary));
        s.push('\n');
        s.push_str(&bar("Memory:", self.memory));
        if let Some(mc) = self.max_context {
            s.push_str(&format!("\n  Max context: {} tokens", fmt_tokens(mc)));
        }
        if self.compression_count > 0 {
            s.push_str(&format!(
                "\n  Compressions: {} | Ratio: {:.2}",
                self.compression_count, self.compression_ratio
            ));
        }
        s
    }
}

fn fmt_tokens(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Cumulative compression metrics for observability.
///
/// Tracks the total number of compression events, tokens saved, messages
/// evicted, and which strategies were used over the lifetime of a
/// [`ContextManager`].
#[derive(Debug, Clone)]
pub struct CompressionMetrics {
    /// Total number of compression events triggered.
    pub total_compressions: u64,
    /// Sum of estimated tokens before compression across all events.
    pub total_tokens_before: u64,
    /// Sum of estimated tokens after compression across all events.
    pub total_tokens_after: u64,
    /// Total number of messages evicted across all events.
    pub total_messages_evicted: u64,
    /// Count of times each compressor type name was used (e.g. "SlidingWindow" → 3).
    pub strategies_used: std::collections::HashMap<String, u64>,
}

impl CompressionMetrics {
    fn new() -> Self {
        Self {
            total_compressions: 0,
            total_tokens_before: 0,
            total_tokens_after: 0,
            total_messages_evicted: 0,
            strategies_used: std::collections::HashMap::new(),
        }
    }

    /// Total tokens saved across all compression events.
    pub fn total_tokens_saved(&self) -> u64 {
        self.total_tokens_before
            .saturating_sub(self.total_tokens_after)
    }

    /// Compression ratio (0.0 = no savings, 1.0 = all tokens saved).
    pub fn compression_ratio(&self) -> f64 {
        if self.total_tokens_before == 0 {
            return 0.0;
        }
        self.total_tokens_saved() as f64 / self.total_tokens_before as f64
    }

    /// Human-readable summary.
    pub fn report(&self) -> String {
        let strategies: Vec<String> = self
            .strategies_used
            .iter()
            .map(|(name, count)| format!("{}({})", name, count))
            .collect();
        format!(
            "CompressionMetrics: {} compressions, {} tokens saved ({:.1}%), {} messages evicted, strategies: [{}]",
            self.total_compressions,
            self.total_tokens_saved(),
            self.compression_ratio() * 100.0,
            self.total_messages_evicted,
            strategies.join(", "),
        )
    }

    /// Record a compression event.
    fn record(&mut self, stats: &ForceCompressStats, compressor_name: &str) {
        self.total_compressions += 1;
        self.total_tokens_before += stats.before_tokens as u64;
        self.total_tokens_after += stats.after_tokens as u64;
        self.total_messages_evicted += stats.evicted as u64;
        *self
            .strategies_used
            .entry(compressor_name.to_string())
            .or_insert(0) += 1;
    }
}

/// Context manager: maintains full conversation history and automatically triggers compression when tokens exceed the limit.
///
/// # Typical usage
///
/// ```rust,no_run
/// use echo_core::error::Result;
/// use echo_core::llm::types::Message;
/// use echo_state::compression::compressor::SlidingWindowCompressor;
/// use echo_state::compression::{ContextCompressor, ContextManager};
///
/// # async fn example() -> Result<()> {
/// let mut ctx = ContextManager::builder(4096)
///     .compressor(SlidingWindowCompressor::new(20))
///     .build();
///
/// ctx.push(Message::system("You are an assistant".to_string()));
/// ctx.push(Message::user("Hello".to_string()));
///
/// // Call prepare() before each LLM call to auto-compress over-limit messages
/// let result = ctx.prepare(None).await?;
/// let messages = result.messages;
/// # Ok(())
/// # }
/// ```
///
/// # SummaryCompressor example (stage4 P4.3 default)
///
/// ```rust,no_run
/// use echo_core::llm::LlmClient;
/// use echo_state::compression::compressor::SummaryCompressor;
/// use echo_state::compression::{ContextCompressor, ContextManager};
/// use std::sync::Arc;
///
/// # async fn example(llm: Arc<dyn LlmClient>) {
/// let compressor = SummaryCompressor::new(llm, 8);
/// let mut ctx = ContextManager::builder(8192)
///     .compressor(compressor)
///     .build();
/// # }
/// ```
pub struct ContextManager {
    messages: Vec<Message>,
    compressor: Option<Box<dyn ContextCompressor>>,
    token_limit: usize,
    tokenizer: Arc<dyn Tokenizer>,
    /// Content markers that identify protected messages (survive compaction).
    /// Any message whose content contains one of these markers is excluded from compression.
    /// Used by the skill system to protect activated skill instructions.
    protected_markers: Vec<String>,
    /// Optional hard message-count cap (supplementary OOM guard).
    ///
    /// (stage4 P4.3) The primary OOM defense is the token budget (`token_limit`
    /// / `budget`); `max_messages` is an optional supplementary guard against
    /// pathological message-count growth (e.g. many tiny messages that don't
    /// trip the token threshold but slow rendering/processing). Industry主流
    /// is token-driven (OpenAI/Anthropic/Letta all bill/window on tokens);
    /// `max_messages` defaults to `None` (disabled) to align with that.
    /// Consumers who want a message-count backstop can set it via
    /// [`ContextManagerBuilder::max_messages`].
    max_messages: Option<usize>,
    /// Optional token budget for percentage-based allocation.
    /// When set, `prepare()` uses budget.allocate() instead of simple token_limit comparison.
    budget: Option<TokenBudget>,
    /// Cumulative compression metrics for observability.
    metrics: CompressionMetrics,
    /// Optional visibility horizon compressor.
    /// When set, `prepare()` runs horizon compaction as a pre-processing pass
    /// before the main compressor, compacting tool traces beyond the active window.
    visibility_horizon: Option<horizon::VisibilityHorizonCompressor>,
    /// Optional callback for promoting evicted messages to long-term memory.
    /// When set, called with evicted messages after each compression pass.
    memory_promoter: Option<Arc<dyn MemoryPromoter>>,
    /// Optional canonical context sources for re-injection after compression.
    /// When set, system prompt, rules, and skill injections can be restored
    /// if compression evicts them from the message buffer.
    canonical_context: Option<CanonicalContext>,
}

impl ContextManager {
    pub fn builder(token_limit: usize) -> ContextManagerBuilder {
        ContextManagerBuilder {
            token_limit,
            compressor: None,
            initial_messages: Vec::new(),
            tokenizer: None,
            max_messages: None,
            budget: None,
            visibility_horizon: None,
            memory_promoter: None,
            canonical_context: None,
        }
    }

    /// Append a message to the context buffer.
    ///
    /// (stage4 P4.3) The primary OOM defense is the token budget (`prepare()`
    /// triggers when estimated tokens exceed `token_limit`). The optional
    /// `max_messages` cap (if set via builder) is a supplementary backstop:
    /// when exceeded, a sliding-window degradation preserves system + protected
    /// + recent messages and discards the earliest in between. Defaults to
    /// `None` (no message-count cap, token-driven, aligned with industry).
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);

        // Optional supplementary hard cap (defaults to None = disabled).
        if let Some(cap) = self.max_messages
            && self.messages.len() > cap
        {
            self.apply_hard_cap(cap);
        }
    }

    /// Apply hard message cap: preserve system messages, protected messages,
    /// and recent messages; discard the earliest in between. Only triggered
    /// when `max_messages` is set and exceeded.
    fn apply_hard_cap(&mut self, target: usize) {
        if self.messages.len() <= target {
            return;
        }

        // Identify protected message indices (should not be deleted).
        let protected: std::collections::HashSet<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter_map(|(i, msg)| {
                if self.is_protected(msg) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();

        // Find the position of the first non-system message
        let first_non_system = self
            .messages
            .iter()
            .position(|m| m.role != Role::System)
            .unwrap_or(0);

        // Preserve: all system messages at the front + the last `target` messages.
        // Discard the earliest non-system, non-protected messages in between.
        let keep_count = self.messages.len() - target;
        let mut to_remove: Vec<usize> = Vec::new();
        let mut removed = 0;
        for i in first_non_system..self.messages.len() {
            if removed >= keep_count {
                break;
            }
            if !protected.contains(&i) && self.messages[i].role != Role::System {
                to_remove.push(i);
                removed += 1;
            }
        }
        // Remove in reverse order to keep indices valid.
        for i in to_remove.into_iter().rev() {
            self.messages.remove(i);
        }
    }

    /// Batch-append messages
    pub fn push_many(&mut self, messages: impl IntoIterator<Item = Message>) {
        self.messages.extend(messages);
    }

    /// Return all messages currently in the buffer (no compression)
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Whether compression is imminent on the next `prepare()` call — i.e. a
    /// compressor is installed AND the token budget is exceeded.
    ///
    /// (stage4 E1) Used by `pre_compaction_flush` to gate the flush LLM call so
    /// it only fires when compaction is actually about to happen, not on every
    /// ReAct iteration. Mirrors `prepare()`'s `needs_compression` decision
    /// (mod.rs:1017-1025) as a non-mutating pre-check. Slight over/under-fire
    /// vs. the budget path is acceptable — the flush is best-effort.
    pub fn should_compress(&self) -> bool {
        if self.compressor.is_none() {
            return false;
        }
        let estimated_tokens = Self::estimate_tokens(&self.messages, &*self.tokenizer);
        if let Some(ref budget) = self.budget {
            let allocation = budget.allocate(0, 0, estimated_tokens);
            allocation.needs_compression()
        } else {
            estimated_tokens > self.token_limit
        }
    }

    /// Replace the internal message buffer (used to restore conversation from persistent storage)
    ///
    /// Messages should include the system prompt as the first entry (if needed).
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// Estimate the token count of the current context
    ///
    /// Uses the configured [`Tokenizer`] implementation (default [`HeuristicTokenizer`], distinguishes ASCII/CJK).
    pub fn token_estimate(&self) -> usize {
        Self::estimate_tokens(&self.messages, &*self.tokenizer)
    }

    /// 获取当前 Tokenizer
    pub fn tokenizer(&self) -> &dyn Tokenizer {
        &*self.tokenizer
    }

    /// Dynamically replace the Tokenizer
    pub fn set_tokenizer(&mut self, tokenizer: Arc<dyn Tokenizer>) {
        self.tokenizer = tokenizer;
    }

    /// Clear the context buffer (preserves configured compressor and protection markers)
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Register a content marker that protects messages from compression.
    ///
    /// Any message whose content contains this marker string will be excluded
    /// from compression passes. This is used by the skill system to protect
    /// activated skill instructions from being evicted during context compaction.
    ///
    /// # Example
    /// ```rust,no_run
    /// # use echo_state::compression::ContextManager;
    /// let mut ctx = ContextManager::builder(4096).build();
    /// ctx.add_protected_marker("<skill_content".to_string());
    /// ```
    pub fn add_protected_marker(&mut self, marker: String) {
        if !self.protected_markers.contains(&marker) {
            self.protected_markers.push(marker);
        }
    }

    /// Replace marker-tagged model context at the system/history boundary.
    ///
    /// Projection messages receive a framework-reserved envelope. Existing
    /// messages with that envelope are removed before current projections are
    /// inserted, making replacement recoverable after persistence and restore.
    pub fn apply_projections(&mut self, projections: &[ContextProjection]) {
        self.messages
            .retain(|message| !is_context_projection_message(message));

        let boundary = self
            .messages
            .iter()
            .position(|message| message.role != Role::System)
            .unwrap_or(self.messages.len());
        let current = projections
            .iter()
            .filter(|projection| !projection.marker.is_empty())
            .filter_map(|projection| {
                projection.message.clone().map(|message| {
                    Self::wrap_projection_message(message, projection.marker.as_str())
                })
            });
        self.messages.splice(boundary..boundary, current);
    }

    /// Count messages currently protected by registered content markers.
    pub fn protected_message_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|message| self.is_protected(message))
            .count()
    }

    /// Return whether the context owns a projection with exactly this marker.
    pub fn has_projection(&self, marker: &str) -> bool {
        let Ok(encoded_marker) = serde_json::to_string(marker) else {
            return false;
        };
        let expected = format!("{PROJECTION_ENVELOPE_PREFIX}\nmarker={encoded_marker}\n");
        self.messages.iter().any(|message| {
            projection_envelope_text(message)
                .is_some_and(|envelope| envelope.starts_with(expected.as_str()))
        })
    }

    /// Check if a message is protected from compression.
    fn is_protected(&self, message: &Message) -> bool {
        if is_context_projection_message(message) {
            return true;
        }
        if self.protected_markers.is_empty() {
            return false;
        }
        if let Some(content) = message.content.as_text() {
            self.protected_markers.iter().any(|m| content.contains(m))
        } else {
            false
        }
    }

    fn wrap_projection_message(mut message: Message, marker: &str) -> Message {
        let encoded_marker =
            serde_json::to_string(marker).unwrap_or_else(|_| "\"invalid-marker\"".to_string());
        let envelope = format!("{PROJECTION_ENVELOPE_PREFIX}\nmarker={encoded_marker}\n");
        match &mut message.content {
            MessageContent::Text(content) => {
                *content = format!("{envelope}{content}");
            }
            MessageContent::Parts(parts) => {
                parts.insert(0, ContentPart::Text { text: envelope });
            }
            MessageContent::Empty => {
                message.content = MessageContent::Text(envelope);
            }
        }
        message
    }

    /// Split messages into (compressible, protected_metadata).
    ///
    /// Protected messages are removed from the compressible set and will be
    /// re-inserted at their original relative positions after compression.
    fn split_protected(&self, messages: Vec<Message>) -> (Vec<Message>, Vec<ProtectedMessage>) {
        let mut compressible = Vec::new();
        let mut protected: Vec<(usize, Message)> = Vec::new();
        let mut compressible_seen = 0usize;

        for msg in messages {
            if self.is_protected(&msg) {
                protected.push((compressible_seen, msg));
            } else {
                compressible.push(msg);
                compressible_seen += 1;
            }
        }

        let total_compressible = compressible.len();
        let total_protected = protected.len();
        let protected = protected
            .into_iter()
            .enumerate()
            .map(|(idx, (compressible_before, message))| ProtectedMessage {
                message,
                compressible_after: total_compressible.saturating_sub(compressible_before),
                protected_after: total_protected.saturating_sub(idx + 1),
            })
            .collect();

        (compressible, protected)
    }

    /// Merge protected messages back into the compressed output.
    ///
    /// Protected messages are re-inserted near their original relative positions.
    /// We restore from the tail so each message can reserve the amount of trailing
    /// conversation that originally followed it.
    fn merge_protected(compressed: Vec<Message>, protected: Vec<ProtectedMessage>) -> Vec<Message> {
        if protected.is_empty() {
            return compressed;
        }

        let mut result = compressed;
        for protected_msg in protected.into_iter().rev() {
            let trailing_slots = protected_msg.compressible_after + protected_msg.protected_after;
            let insert_at = result.len().saturating_sub(trailing_slots);
            result.insert(insert_at, protected_msg.message);
        }
        result
    }

    /// Dynamically replace the compressor without affecting the existing message buffer
    pub fn set_compressor(&mut self, compressor: impl ContextCompressor + 'static) {
        self.compressor = Some(Box::new(compressor));
    }

    /// Set or replace the visibility horizon compressor.
    ///
    /// When configured, `prepare()` runs horizon compaction as a pre-processing
    /// pass before the main compressor, compacting tool traces beyond the
    /// active plan window.
    pub fn set_visibility_horizon(&mut self, compressor: horizon::VisibilityHorizonCompressor) {
        self.visibility_horizon = Some(compressor);
    }

    /// Remove the visibility horizon compressor.
    pub fn remove_visibility_horizon(&mut self) {
        self.visibility_horizon = None;
    }

    /// Set or replace the memory promoter.
    ///
    /// When set, the promoter receives evicted messages after each
    /// compression pass, enabling L3 memory promotion.
    pub fn set_memory_promoter(&mut self, promoter: Arc<dyn MemoryPromoter>) {
        self.memory_promoter = Some(promoter);
    }

    /// Remove the memory promoter.
    pub fn remove_memory_promoter(&mut self) {
        self.memory_promoter = None;
    }

    /// Run memory promotion and tool-call sanitization on the internal buffer.
    ///
    /// Shared by `prepare()` and `force_compress*()` to ensure consistent
    /// post-compression processing: evicted facts → long-term memory, and
    /// broken tool-call pairs → fixed.
    ///
    /// Returns the number of evicted messages sent to the promoter.
    async fn promote_and_sanitize(&mut self, evicted_messages: &[Message]) -> usize {
        // ── Memory promotion ──
        let count = if let Some(ref promoter) = self.memory_promoter {
            if !evicted_messages.is_empty() {
                promoter.promote(evicted_messages).await;
                evicted_messages.len()
            } else {
                0
            }
        } else {
            0
        };

        // ── Tool-call sanitization ──
        let (sanitized, _fixes) = sanitize_tool_call_pairing(&self.messages);
        self.messages = sanitized;

        count
    }

    /// Set canonical context sources for re-injection after compression.
    ///
    /// When set, `prepare()` will check if compression evicted critical context
    /// (system prompt, rules, skill injections) and re-inject them if needed.
    pub fn set_canonical_context(&mut self, context: CanonicalContext) {
        self.canonical_context = Some(context);
    }

    /// Update only the canonical system prompt while preserving rules and skills.
    pub fn set_canonical_system_prompt(&mut self, system_prompt: Option<String>) {
        if let Some(context) = &mut self.canonical_context {
            context.system_prompt = system_prompt;
        }
    }

    /// Remove the canonical context.
    pub fn remove_canonical_context(&mut self) {
        self.canonical_context = None;
    }

    /// Re-inject canonical context if compression removed critical components.
    ///
    /// Called automatically at the end of `prepare()` when compression occurs.
    fn reinject_canonical_context(&mut self) {
        let Some(ref canonical) = self.canonical_context else {
            return;
        };

        // Restore the exact canonical prompt even when another system message
        // occupies the system region.
        if let Some(ref prompt) = canonical.system_prompt {
            let has_canonical_system = self.messages.iter().any(|message| {
                message.role == Role::System
                    && message.content.as_text_ref() == Some(prompt.as_str())
            });
            if !has_canonical_system {
                self.messages.insert(0, Message::system(prompt.clone()));
                tracing::debug!("Re-injected system prompt from canonical context");
            }
        }

        // Inject canonical context messages (system prompt, rules, skills).
        //
        // IMPORTANT: insert at the end of the system region (sys_end), NOT at a
        // fixed position like 1. Inserting at pos=1 shifts the entire conversation
        // history, which invalidates the prompt cache prefix on every compression
        // cycle. By inserting at sys_end (the boundary between system messages and
        // history), we keep the history segment's byte positions stable, preserving
        // both Anthropic explicit cache breakpoints and OpenAI automatic prefix
        // caches.
        if let Some(msgs) = canonical.to_reinjection_messages() {
            // Find the end of the system message region (first non-System role).
            let sys_end = self
                .messages
                .iter()
                .position(|m| m.role != Role::System)
                .unwrap_or(self.messages.len());

            // Dedup: skip canonical messages whose text already exists in the
            // current system region (prevents accumulation on repeated compression).
            let existing_texts: std::collections::HashSet<String> = self.messages[..sys_end]
                .iter()
                .filter_map(|m| m.content.as_text())
                .collect();
            let to_insert: Vec<String> = msgs
                .into_iter()
                .filter(|msg| !existing_texts.contains(msg.as_str()))
                .collect();

            // Insert at sys_end (tight against history), keeping history stable.
            // Iterate in reverse so each message lands at the correct offset.
            for msg in to_insert.into_iter().rev() {
                self.messages.insert(sys_end, Message::system(msg));
            }
        }
    }

    fn finalize_checkpoint(
        &self,
        checkpoint: Option<CompressionCheckpoint>,
    ) -> Option<CompressionCheckpoint> {
        checkpoint.map(|mut value| {
            value.retained_count = self.messages.len();
            value.token_after = self.token_estimate();
            value
        })
    }

    /// Remove the compressor, reverting to unlimited mode
    pub fn remove_compressor(&mut self) {
        self.compressor = None;
    }

    /// Whether a compressor is configured
    pub fn has_compressor(&self) -> bool {
        self.compressor.is_some()
    }

    /// Get cumulative compression metrics for observability.
    ///
    /// Tracks total compressions, tokens saved, messages evicted, and
    /// which strategies were used over the lifetime of this `ContextManager`.
    pub fn compression_metrics(&self) -> &CompressionMetrics {
        &self.metrics
    }

    /// Reset cumulative compression metrics to zero.
    pub fn reset_compression_metrics(&mut self) {
        self.metrics = CompressionMetrics::new();
    }

    /// Compute a token breakdown by message role.
    ///
    /// Useful for `/context` visualization — shows how context window budget is
    /// distributed across system prompts, user messages, tool outputs, etc.
    pub fn token_breakdown(&self, max_context: Option<usize>) -> TokenBreakdown {
        let tokenizer = &*self.tokenizer;
        let mut system = 0;
        let mut user = 0;
        let mut assistant = 0;
        let mut tool = 0;
        let mut summary = 0;
        let mut memory = 0;

        for msg in &self.messages {
            let text = msg.content.as_text().unwrap_or_default();
            let tokens = tokenizer.count_tokens(&text);
            match msg.role {
                Role::System => {
                    if text.contains("[对话历史摘要]") {
                        summary += tokens;
                    } else if text.contains("[Relevant historical memories]")
                        || text.contains("[Related historical memories]")
                    {
                        memory += tokens;
                    } else {
                        system += tokens;
                    }
                }
                Role::User => user += tokens,
                Role::Assistant => assistant += tokens,
                Role::Tool => tool += tokens,
                _ => {}
            }
        }

        let total = system + user + assistant + tool + summary + memory;
        let compression_ratio = self.metrics.compression_ratio();

        TokenBreakdown {
            system,
            user,
            assistant,
            tool,
            summary,
            memory,
            total,
            max_context,
            token_limit: self.token_limit,
            compression_count: self.metrics.total_compressions,
            compression_ratio,
        }
    }

    /// Force-compress the context, regardless of whether the current token count exceeds the limit.
    ///
    /// - If a compressor is configured, use it;
    /// - Otherwise, temporarily use `SlidingWindowCompressor::new(fallback_window)`.
    ///
    /// Protected messages are excluded from compression and preserved.
    pub async fn force_compress(
        &mut self,
        fallback_window: usize,
    ) -> Result<(ForceCompressStats, Option<CompressionCheckpoint>)> {
        let before_count = self.messages.len();
        let before_tokens = self.token_estimate();

        let (compressible, protected) = self.split_protected(self.messages.clone());

        let output = if let Some(compressor) = &self.compressor {
            compressor
                .compress(CompressionInput {
                    messages: compressible,
                    token_limit: self.token_limit,
                    current_query: None,
                    focus_instructions: None,
                })
                .await?
        } else {
            SlidingWindowCompressor::new(fallback_window)
                .compress(CompressionInput {
                    messages: compressible,
                    token_limit: self.token_limit,
                    current_query: None,
                    focus_instructions: None,
                })
                .await?
        };

        let checkpoint = output
            .checkpoint
            .map(|cp| cp.with_protected_count(protected.len()));

        let evicted_messages = output.evicted;
        let evicted = evicted_messages.len();
        self.messages = Self::merge_protected(output.messages, protected);

        // ── Memory promotion + sanitize ──
        let memory_promotion_count = self.promote_and_sanitize(&evicted_messages).await;

        let checkpoint =
            checkpoint.map(|cp| cp.with_memory_promotion_count(memory_promotion_count));

        if self.canonical_context.is_some() {
            self.reinject_canonical_context();
        }
        let checkpoint = self.finalize_checkpoint(checkpoint);

        let stats = ForceCompressStats {
            before_count,
            after_count: self.messages.len(),
            evicted,
            before_tokens,
            after_tokens: self.token_estimate(),
        };
        let name = if self.compressor.is_some() {
            self.compressor
                .as_ref()
                .map(|c| c.name())
                .unwrap_or("unknown")
        } else {
            "SlidingWindow(fallback)"
        };
        self.metrics.record(&stats, name);
        Ok((stats, checkpoint))
    }

    /// Force-compress with user-provided focus instructions.
    ///
    /// The focus instructions are passed to the compressor via `CompressionInput::current_query`,
    /// allowing LLM-based compressors (Summary, IncrementalSummary, Adaptive L4) to
    /// prioritize specific topics in their summaries.
    pub async fn force_compress_with_focus(
        &mut self,
        focus_instructions: &str,
        fallback_window: usize,
    ) -> Result<(ForceCompressStats, Option<CompressionCheckpoint>)> {
        let before_count = self.messages.len();
        let before_tokens = self.token_estimate();

        let (compressible, protected) = self.split_protected(self.messages.clone());

        let output = if let Some(compressor) = &self.compressor {
            compressor
                .compress(CompressionInput {
                    messages: compressible,
                    token_limit: self.token_limit,
                    current_query: None,
                    focus_instructions: Some(focus_instructions.to_string()),
                })
                .await?
        } else {
            SlidingWindowCompressor::new(fallback_window)
                .compress(CompressionInput {
                    messages: compressible,
                    token_limit: self.token_limit,
                    current_query: None,
                    focus_instructions: Some(focus_instructions.to_string()),
                })
                .await?
        };

        let checkpoint = output
            .checkpoint
            .map(|cp| cp.with_protected_count(protected.len()));

        let evicted_messages = output.evicted;
        let evicted = evicted_messages.len();
        self.messages = Self::merge_protected(output.messages, protected);

        // ── Memory promotion + sanitize ──
        let memory_promotion_count = self.promote_and_sanitize(&evicted_messages).await;

        let checkpoint =
            checkpoint.map(|cp| cp.with_memory_promotion_count(memory_promotion_count));

        if self.canonical_context.is_some() {
            self.reinject_canonical_context();
        }
        let checkpoint = self.finalize_checkpoint(checkpoint);

        let stats = ForceCompressStats {
            before_count,
            after_count: self.messages.len(),
            evicted,
            before_tokens,
            after_tokens: self.token_estimate(),
        };
        let name = if self.compressor.is_some() {
            self.compressor
                .as_ref()
                .map(|c| c.name())
                .unwrap_or("unknown")
        } else {
            "SlidingWindow(fallback)"
        };
        self.metrics.record(&stats, name);
        Ok((stats, checkpoint))
    }

    /// Force-compress using a **specific compressor**, without affecting the currently installed compressor config.
    ///
    /// Suitable for temporary strategy overrides like `/compress sliding 10`.
    pub async fn force_compress_with(
        &mut self,
        compressor: &dyn ContextCompressor,
    ) -> Result<(ForceCompressStats, Option<CompressionCheckpoint>)> {
        let before_count = self.messages.len();
        let before_tokens = self.token_estimate();

        let (compressible, protected) = self.split_protected(self.messages.clone());

        let output = compressor
            .compress(CompressionInput {
                messages: compressible,
                token_limit: self.token_limit,
                current_query: None,
                focus_instructions: None,
            })
            .await?;

        let checkpoint = output
            .checkpoint
            .map(|cp| cp.with_protected_count(protected.len()));

        let evicted_messages = output.evicted;
        let evicted = evicted_messages.len();
        self.messages = Self::merge_protected(output.messages, protected);

        // ── Memory promotion + sanitize ──
        let memory_promotion_count = self.promote_and_sanitize(&evicted_messages).await;

        let checkpoint =
            checkpoint.map(|cp| cp.with_memory_promotion_count(memory_promotion_count));

        if self.canonical_context.is_some() {
            self.reinject_canonical_context();
        }
        let checkpoint = self.finalize_checkpoint(checkpoint);

        let stats = ForceCompressStats {
            before_count,
            after_count: self.messages.len(),
            evicted,
            before_tokens,
            after_tokens: self.token_estimate(),
        };
        self.metrics.record(&stats, compressor.name());
        Ok((stats, checkpoint))
    }

    /// Update the system message content
    ///
    /// Typically called when `add_skill()` injects extra system prompts:
    /// finds the first message with role == "system" and replaces its content;
    /// if no system message exists, inserts one at the head of the queue.
    pub fn update_system(&mut self, new_system_prompt: String) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.role == Role::System) {
            msg.content = MessageContent::Text(new_system_prompt);
        } else {
            self.messages.insert(0, Message::system(new_system_prompt));
        }
    }

    /// Prepare the list of messages to send to the LLM.
    ///
    /// When the estimated token count exceeds `token_limit` and a compressor is configured, automatically trigger compression and update the internal buffer.
    /// The compressed messages replace the original buffer.
    ///
    /// Protected messages (containing registered markers, e.g. `<skill_content>`) are
    /// excluded from compression and re-inserted after system messages.
    ///
    /// `current_query` is a reserved field; pass `None`.
    ///
    /// Returns a [`PrepareResult`] containing the prepared messages and optional
    /// compression stats (populated only when auto-compression was triggered).
    pub async fn prepare(&mut self, current_query: Option<&str>) -> Result<PrepareResult> {
        // ── Snapshot original messages for verification ──
        let original_messages = self.messages.clone();

        // ── Pre-compression: Visibility Horizon pass ──────────────────
        // Compact tool traces beyond the active window before the main
        // compressor runs. This reduces the token count so the main
        // compressor may not need to fire at all.
        if let Some(ref horizon_compressor) = self.visibility_horizon {
            let before = self.messages.len();
            let horizon_input = CompressionInput {
                messages: std::mem::take(&mut self.messages),
                token_limit: self.token_limit,
                current_query: current_query.map(String::from),
                focus_instructions: None,
            };
            match horizon_compressor.compress(horizon_input).await {
                Ok(output) => {
                    let compacted = before.saturating_sub(output.messages.len());
                    let evicted_count = output.evicted.len();
                    if evicted_count > 0 {
                        if let Some(ref promoter) = self.memory_promoter {
                            promoter.promote(&output.evicted).await;
                        }
                    }
                    if compacted > 0 {
                        tracing::debug!(
                            before_messages = before,
                            after_messages = output.messages.len(),
                            evicted = evicted_count,
                            "VisibilityHorizon pre-compaction applied"
                        );
                        self.metrics.record(
                            &ForceCompressStats {
                                before_count: before,
                                after_count: output.messages.len(),
                                evicted: output.evicted.len(),
                                before_tokens: 0,
                                after_tokens: 0,
                            },
                            "VisibilityHorizon",
                        );
                    }
                    self.messages = output.messages;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "VisibilityHorizon pre-compaction failed, continuing with original messages");
                    // Don't fail prepare — horizon is best-effort
                }
            }
        }

        let estimated_tokens = Self::estimate_tokens(&self.messages, &*self.tokenizer);

        // Compute effective token limit once so primary compression and any
        // verifier fallback obey the same budget-aware allowance.
        let effective_limit = if let Some(ref budget) = self.budget {
            let allocation = budget.allocate(0, 0, estimated_tokens);
            (estimated_tokens.saturating_sub(allocation.conversation_excess))
                .max(self.token_limit / 2)
        } else {
            self.token_limit
        };

        let needs_compression = if let Some(ref budget) = self.budget {
            // Budget-aware check: use percentage-based allocation
            let system_tokens = 0; // system prompt tokens already counted in messages
            let tool_tokens = 0; // tool defs not in messages
            let allocation = budget.allocate(system_tokens, tool_tokens, estimated_tokens);
            allocation.needs_compression()
        } else {
            estimated_tokens > self.token_limit
        };

        let (compressed, mut combined_checkpoint) = if let Some(compressor) = &self.compressor
            && needs_compression
        {
            let before_count = self.messages.len();
            let before_tokens = self.token_estimate();
            let compressor_name = compressor.name();

            tracing::debug!(
                before_messages = before_count,
                before_tokens,
                token_limit = self.token_limit,
                compressor = compressor_name,
                "Auto-compression triggered"
            );
            let start = std::time::Instant::now();

            // Use the budget-aware effective token limit computed above.

            let owned = std::mem::take(&mut self.messages);
            let (compressible, protected) = self.split_protected(owned);

            let compress_result = compressor
                .compress(CompressionInput {
                    messages: compressible.clone(),
                    token_limit: effective_limit,
                    current_query: current_query.map(String::from),
                    focus_instructions: None,
                })
                .await;

            match compress_result {
                Ok(output) => {
                    let evicted_messages = output.evicted;
                    let evicted = evicted_messages.len();
                    let compressor_checkpoint = output.checkpoint;
                    let protected_count = protected.len();
                    self.messages = Self::merge_protected(output.messages, protected);

                    // ── L3 Memory Promotion ──
                    // If a memory promoter is configured, pass evicted messages
                    // so key facts can be extracted and stored for later recall.
                    let memory_promotion_count = if let Some(ref promoter) = self.memory_promoter {
                        if !evicted_messages.is_empty() {
                            promoter.promote(&evicted_messages).await;
                            evicted_messages.len()
                        } else {
                            0
                        }
                    } else {
                        0
                    };

                    let after_tokens = self.token_estimate();
                    let stats = ForceCompressStats {
                        before_count,
                        after_count: self.messages.len(),
                        evicted,
                        before_tokens,
                        after_tokens,
                    };
                    let elapsed = start.elapsed();
                    self.metrics.record(&stats, compressor_name);

                    tracing::info!(
                        compressor = compressor_name,
                        before_messages = stats.before_count,
                        after_messages = stats.after_count,
                        before_tokens = stats.before_tokens,
                        after_tokens = stats.after_tokens,
                        evicted = stats.evicted,
                        saved_tokens = stats.before_tokens.saturating_sub(stats.after_tokens),
                        elapsed_ms = elapsed.as_millis() as u64,
                        "Compression complete"
                    );

                    // Assemble the checkpoint from the compressor's output
                    let compressed_checkpoint = compressor_checkpoint.map(|cp| {
                        cp.with_protected_count(protected_count)
                            .with_memory_promotion_count(memory_promotion_count)
                    });

                    (Some(stats), compressed_checkpoint)
                }
                Err(e) => {
                    // Primary compressor failed — fall back to SlidingWindow as safety net
                    // to avoid losing all non-protected messages.
                    tracing::warn!(
                        error = %e,
                        "Primary compressor failed, falling back to SlidingWindowCompressor"
                    );
                    let fallback = SlidingWindowCompressor::new(40);
                    match fallback
                        .compress(CompressionInput {
                            messages: compressible,
                            token_limit: effective_limit,
                            current_query: current_query.map(String::from),
                            focus_instructions: None,
                        })
                        .await
                    {
                        Ok(fb_output) => {
                            self.messages = Self::merge_protected(fb_output.messages, protected);
                            let stats = ForceCompressStats {
                                before_count,
                                after_count: self.messages.len(),
                                evicted: fb_output.evicted.len(),
                                before_tokens,
                                after_tokens: self.token_estimate(),
                            };
                            self.metrics.record(&stats, "SlidingWindow(fallback)");
                            // Return the fallback result, not the original error
                            (Some(stats), fb_output.checkpoint)
                        }
                        Err(fb_err) => {
                            // Even the fallback failed — restore the original buffer before
                            // returning so callers that retry do not observe truncated state.
                            self.messages = original_messages.clone();
                            tracing::error!(
                                primary_error = %e,
                                fallback_error = %fb_err,
                                "Both primary compressor and SlidingWindow fallback failed — some conversation context lost"
                            );
                            return Err(e);
                        }
                    }
                }
            }
        } else {
            (None, None)
        };

        // Always sanitize tool_calls → tool_result pairing before sending to LLM.
        // Even without compression, session resume or manual manipulation can
        // produce invalid sequences.
        let (sanitized, tool_fixes) = sanitize_tool_call_pairing(&self.messages);
        // Write sanitized messages back to the internal buffer so subsequent
        // reads see consistent state, and canonical re-injection targets them.
        self.messages = sanitized;

        // Merge tool_pair_fixes into the compression checkpoint
        if let Some(ref mut cp) = combined_checkpoint {
            cp.tool_pair_fixes = tool_fixes;
        }

        // ── Summary verification ──
        // Run lightweight rule-based checks when a summary was produced.
        // On P0 check failure, fall back to SlidingWindowCompressor as safety net.
        let verification = if let Some(ref cp) = combined_checkpoint {
            if cp.summary.is_some() {
                let v = verifier::verify_compression(&self.messages, cp, &original_messages);
                if !v.passed {
                    let p0_failed: Vec<&str> = v
                        .checks
                        .iter()
                        .filter(|c| !c.passed && c.priority == verifier::CheckPriority::P0)
                        .map(|c| c.name.as_str())
                        .collect();
                    if !p0_failed.is_empty() {
                        tracing::warn!(
                            failed_checks = ?p0_failed,
                            "Summary verifier: P0 checks FAILED — falling back to SlidingWindowCompressor to recover critical information"
                        );
                        // Re-compress original messages with SlidingWindow as safety net
                        let (orig_compressible, orig_protected) =
                            self.split_protected(original_messages.clone());
                        if let Ok(fb_output) = SlidingWindowCompressor::new(40)
                            .compress(CompressionInput {
                                messages: orig_compressible,
                                token_limit: effective_limit,
                                current_query: current_query.map(String::from),
                                focus_instructions: None,
                            })
                            .await
                        {
                            self.messages =
                                Self::merge_protected(fb_output.messages, orig_protected);
                            // Re-sanitize after fallback
                            let (sanitized, _fb_fixes) = sanitize_tool_call_pairing(&self.messages);
                            self.messages = sanitized;
                            // Update the checkpoint to note the fallback
                            let fb_checkpoint =
                                CompressionCheckpoint::new("SlidingWindow(verifier-fallback)")
                                    .with_counts(self.messages.len(), fb_output.evicted.len())
                                    .with_tokens(
                                        Self::estimate_tokens(&original_messages, &*self.tokenizer),
                                        self.token_estimate(),
                                    );
                            combined_checkpoint = Some(fb_checkpoint);
                            tracing::info!(
                                after_messages = self.messages.len(),
                                after_tokens = self.token_estimate(),
                                "Recovered via SlidingWindow fallback after P0 verification failure"
                            );
                        } else {
                            tracing::error!(
                                "SlidingWindow fallback also failed after P0 verification failure — continuing with current compressed messages"
                            );
                        }
                    }
                }
                Some(v)
            } else {
                None
            }
        } else {
            None
        };

        // ── Canonical context re-injection ──
        // If canonical context is configured, ensure critical components
        // (system prompt, rules, skill info) survived compression.
        if self.canonical_context.is_some() {
            self.reinject_canonical_context();
        }
        combined_checkpoint = self.finalize_checkpoint(combined_checkpoint);

        Ok(PrepareResult {
            messages: self.messages.clone(),
            compressed,
            checkpoint: combined_checkpoint,
            verification,
        })
    }

    fn estimate_tokens(messages: &[Message], tokenizer: &dyn Tokenizer) -> usize {
        messages
            .iter()
            .filter_map(|m| m.content.as_text())
            .map(|c| tokenizer.count_tokens(&c))
            .sum()
    }
}

/// Sanitize message sequence to ensure valid `tool_calls → tool_result` pairing.
///
/// OpenAI-compatible APIs require that:
/// - Every `assistant` message with `tool_calls` must be followed by `tool` messages
///   for each `tool_call_id`
/// - Every `tool` message must have a preceding `assistant` message with matching
///   `tool_call_id`
///
/// Compression, session resume, or manual manipulation can violate these constraints.
/// This function repairs the sequence by:
/// 1. Adding placeholder `tool` results for orphaned `tool_calls`
/// 2. Removing orphaned `tool` messages without matching `tool_calls`
fn sanitize_tool_call_pairing(messages: &[Message]) -> (Vec<Message>, Vec<ToolPairFix>) {
    use std::collections::{HashMap, HashSet};

    let mut fixes: Vec<ToolPairFix> = Vec::new();

    if messages.is_empty() {
        return (vec![], fixes);
    }

    // Pass 1: Count tool_call_ids per assistant message and collect all available results
    // Key: assistant message index, Value: set of tool_call_ids in that message
    let mut assistant_tool_calls: HashMap<usize, HashSet<String>> = HashMap::new();
    // All tool_call_ids that have corresponding tool result messages
    let mut available_results: HashSet<String> = HashSet::new();

    for (i, msg) in messages.iter().enumerate() {
        if msg.role == Role::Assistant {
            if let Some(ref tcs) = msg.tool_calls {
                let ids: HashSet<String> = tcs.iter().map(|tc| tc.id.clone()).collect();
                assistant_tool_calls.insert(i, ids);
            }
        } else if msg.role == Role::Tool {
            if let Some(ref id) = msg.tool_call_id {
                available_results.insert(id.clone());
            }
        }
    }

    // If no tool_calls at all, return as-is
    if assistant_tool_calls.is_empty() && available_results.is_empty() {
        return (messages.to_vec(), fixes);
    }

    // Build the set of all referenced tool_call_ids (from assistant messages)
    let all_referenced: HashSet<String> = assistant_tool_calls
        .values()
        .flat_map(|ids| ids.iter().cloned())
        .collect();

    // Pass 2: Build sanitized sequence
    let mut result = Vec::with_capacity(messages.len());
    let mut inserted_placeholders: HashSet<String> = HashSet::new();
    // Track which assistant message's placeholders are pending (inserted after
    // all its original tool results have been seen)
    let mut pending_placeholders: Vec<Vec<String>> = Vec::new();

    for (i, msg) in messages.iter().enumerate() {
        // Before processing the current message, check if any pending placeholder
        // groups should be flushed. We flush when we encounter a non-tool message
        // (meaning the tool result sequence for the previous assistant has ended).
        if msg.role != Role::Tool && !pending_placeholders.is_empty() {
            for ids in pending_placeholders.drain(..) {
                for id in ids {
                    if !inserted_placeholders.contains(&id) {
                        result.push(Message::tool_result(
                            id.clone(),
                            "unknown".to_string(),
                            "[Result unavailable — tool result was removed during context compression]".to_string(),
                        ));
                        fixes.push(ToolPairFix {
                            tool_call_id: id.clone(),
                            fix_type: ToolPairFixType::PlaceholderResultInserted,
                        });
                        inserted_placeholders.insert(id);
                    }
                }
            }
        }

        if msg.role == Role::Assistant {
            if let Some(tc_ids) = assistant_tool_calls.get(&i) {
                // Check if ALL tool_call_ids in this message are orphaned
                let all_orphaned = tc_ids.iter().all(|id| !available_results.contains(id));

                if all_orphaned {
                    // Remove tool_calls field — treat as regular assistant message
                    let mut cleaned = msg.clone();
                    for tc_id in tc_ids.iter() {
                        fixes.push(ToolPairFix {
                            tool_call_id: tc_id.clone(),
                            fix_type: ToolPairFixType::DanglingCallCleared,
                        });
                    }
                    cleaned.tool_calls = None;
                    result.push(cleaned);
                } else {
                    result.push(msg.clone());
                    // Track which IDs need placeholders (will be inserted after
                    // all original tool results for this assistant message)
                    let missing: Vec<String> = tc_ids
                        .iter()
                        .filter(|id| !available_results.contains(*id))
                        .cloned()
                        .collect();
                    if !missing.is_empty() {
                        pending_placeholders.push(missing);
                    }
                }
            } else {
                result.push(msg.clone());
            }
        } else if msg.role == Role::Tool {
            // Only include if the tool_call_id is referenced by an assistant message
            let is_orphaned = match &msg.tool_call_id {
                Some(id) => !all_referenced.contains(id),
                None => true, // Tool message without tool_call_id is always orphaned
            };

            if !is_orphaned {
                result.push(msg.clone());
            } else {
                fixes.push(ToolPairFix {
                    tool_call_id: msg
                        .tool_call_id
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                    fix_type: ToolPairFixType::OrphanedResultRemoved,
                });
                tracing::debug!(
                    tool_call_id = msg.tool_call_id.as_deref().unwrap_or("<none>"),
                    "Removed orphaned tool result message"
                );
            }
        } else {
            result.push(msg.clone());
        }
    }

    // Flush any remaining pending placeholders (end of message list)
    for ids in pending_placeholders {
        for id in ids {
            if !inserted_placeholders.contains(&id) {
                result.push(Message::tool_result(
                    id.clone(),
                    "unknown".to_string(),
                    "[Result unavailable — tool result was removed during context compression]"
                        .to_string(),
                ));
                fixes.push(ToolPairFix {
                    tool_call_id: id,
                    fix_type: ToolPairFixType::PlaceholderResultInserted,
                });
            }
        }
    }

    (result, fixes)
}

/// Builder for `ContextManager`
pub struct ContextManagerBuilder {
    token_limit: usize,
    compressor: Option<Box<dyn ContextCompressor>>,
    initial_messages: Vec<Message>,
    tokenizer: Option<Arc<dyn Tokenizer>>,
    max_messages: Option<usize>,
    budget: Option<TokenBudget>,
    visibility_horizon: Option<horizon::VisibilityHorizonCompressor>,
    memory_promoter: Option<Arc<dyn MemoryPromoter>>,
    canonical_context: Option<CanonicalContext>,
}

impl ContextManagerBuilder {
    /// Set the compression strategy (optional). Supports any type implementing `ContextCompressor`,
    /// including `SlidingWindowCompressor` and `SummaryCompressor` (stage4 P4.3 default).
    pub fn compressor(mut self, c: impl ContextCompressor + 'static) -> Self {
        self.compressor = Some(Box::new(c));
        self
    }

    /// Pre-set a system message as the initial context (typically used for Agent system prompts)
    pub fn with_system(mut self, system_prompt: String) -> Self {
        self.initial_messages.push(Message::system(system_prompt));
        self
    }

    /// Set a custom Tokenizer (default [`HeuristicTokenizer`])
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_state::compression::ContextManager;
    /// use echo_core::tokenizer::SimpleTokenizer;
    /// use std::sync::Arc;
    ///
    /// let ctx = ContextManager::builder(4096)
    ///     .tokenizer(Arc::new(SimpleTokenizer))
    ///     .build();
    /// ```
    pub fn tokenizer(mut self, tokenizer: Arc<dyn Tokenizer>) -> Self {
        self.tokenizer = Some(tokenizer);
        self
    }

    /// Set an optional hard message-count cap (supplementary OOM backstop).
    ///
    /// (stage4 P4.3) The primary OOM defense is the token budget; industry主流
    /// is token-driven (OpenAI/Anthropic/Letta all bill/window on tokens).
    /// `max_messages` defaults to `None` (disabled). Set it only when you need
    /// a message-count backstop against pathological growth (e.g. many tiny
    /// messages). When exceeded, `push()` applies sliding-window degradation
    /// preserving system + protected + recent messages.
    pub fn max_messages(mut self, max: usize) -> Self {
        self.max_messages = Some(max);
        self
    }

    /// Set a token budget for percentage-based allocation.
    ///
    /// When set, `prepare()` uses budget-aware compression instead of
    /// the simple `token_limit` comparison. The budget divides the context
    /// window into system/tool/output/safety percentages and allocates
    /// the remainder for conversation.
    pub fn budget(mut self, budget: TokenBudget) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Set a visibility horizon for proactive tool trace compaction.
    ///
    /// When set, `prepare()` runs a horizon compaction pass **before** the
    /// main compressor. Tool call/result groups beyond the active window
    /// are replaced with compact symbolic summaries, keeping the context
    /// focused on recent work.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_state::compression::{ContextManager, horizon::VisibilityHorizonConfig};
    ///
    /// let ctx = ContextManager::builder(128_000)
    ///     .visibility_horizon(VisibilityHorizonConfig {
    ///         active_window_turns: 5,
    ///         ..Default::default()
    ///     })
    ///     .build();
    /// ```
    pub fn visibility_horizon(mut self, config: horizon::VisibilityHorizonConfig) -> Self {
        self.visibility_horizon = Some(horizon::VisibilityHorizonCompressor::new(config));
        self
    }

    /// Set a memory promoter callback.
    ///
    /// When compression evicts messages, the promoter is called with the
    /// evicted messages so key facts can be extracted and written to a
    /// long-term memory Store (L3 memory promotion).
    pub fn memory_promoter(mut self, promoter: Arc<dyn MemoryPromoter>) -> Self {
        self.memory_promoter = Some(promoter);
        self
    }

    /// Set the canonical context for re-injection after compression.
    pub fn canonical_context(mut self, context: CanonicalContext) -> Self {
        self.canonical_context = Some(context);
        self
    }

    pub fn build(self) -> ContextManager {
        ContextManager {
            messages: self.initial_messages,
            compressor: self.compressor,
            token_limit: self.token_limit,
            tokenizer: self
                .tokenizer
                .unwrap_or_else(|| Arc::new(HeuristicTokenizer)),
            // (stage4 G1) `protected_memory` is a default marker so recalled
            // memories (wrapped by `format_memory_context`) survive compaction.
            protected_markers: vec!["protected_memory".to_string()],
            max_messages: self.max_messages,
            budget: self.budget,
            metrics: CompressionMetrics::new(),
            visibility_horizon: self.visibility_horizon,
            memory_promoter: self.memory_promoter,
            canonical_context: self.canonical_context,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::compressor::SlidingWindowCompressor;
    use echo_core::error::Result;

    #[test]
    fn context_projection_replaces_existing_tagged_message() {
        let marker = "<runtime_projection>";
        let mut ctx = ContextManager::builder(4096)
            .with_system("system".to_string())
            .build();
        ctx.push(Message::user("history".to_string()));

        ctx.apply_projections(&[ContextProjection {
            marker: marker.to_string(),
            message: Some(Message::user(format!("{marker}old"))),
        }]);
        ctx.apply_projections(&[ContextProjection {
            marker: marker.to_string(),
            message: Some(Message::user(format!("{marker}new"))),
        }]);

        let projected: Vec<_> = ctx
            .messages()
            .iter()
            .filter(|message| {
                message
                    .content
                    .as_text_ref()
                    .is_some_and(|text| text.contains(marker))
            })
            .collect();
        assert_eq!(projected.len(), 1);
        assert!(
            projected
                .first()
                .is_some_and(|message| is_context_projection_message(message))
        );
        assert!(
            projected
                .first()
                .and_then(|message| message.content.as_text_ref())
                .is_some_and(|text| text.ends_with("<runtime_projection>new"))
        );
        assert_eq!(ctx.protected_message_count(), 1);
    }

    #[test]
    fn context_projection_none_removes_stale_tagged_message() {
        let marker = "<runtime_projection>";
        let mut ctx = ContextManager::builder(4096)
            .with_system("system".to_string())
            .build();
        ctx.apply_projections(&[ContextProjection {
            marker: marker.to_string(),
            message: Some(Message::user(format!("{marker}stale"))),
        }]);

        ctx.apply_projections(&[ContextProjection {
            marker: marker.to_string(),
            message: None,
        }]);

        assert!(
            ctx.messages().iter().all(|message| {
                message
                    .content
                    .as_text_ref()
                    .is_none_or(|text| !text.contains(marker))
            }),
            "None projection must remove stale tagged context"
        );
        assert_eq!(ctx.protected_message_count(), 0);
    }

    #[test]
    fn context_projection_preserves_unrelated_marker_text() {
        let marker = "runtime-marker";
        let mut ctx = ContextManager::builder(4096)
            .with_system("system".to_string())
            .build();
        ctx.push(Message::user(
            "ordinary conversation mentions runtime-marker".to_string(),
        ));

        ctx.apply_projections(&[ContextProjection {
            marker: marker.to_string(),
            message: Some(Message::user("projected state".to_string())),
        }]);

        assert!(ctx.messages().iter().any(|message| {
            message
                .content
                .as_text_ref()
                .is_some_and(|text| text == "ordinary conversation mentions runtime-marker")
        }));
        assert_eq!(ctx.protected_message_count(), 1);
    }

    #[test]
    fn has_projection_matches_exact_envelope_marker() {
        let mut ctx = ContextManager::builder(4096).build();
        ctx.apply_projections(&[ContextProjection {
            marker: "runtime-marker".to_string(),
            message: Some(Message::user("current runtime".to_string())),
        }]);

        assert!(ctx.has_projection("runtime-marker"));
        assert!(!ctx.has_projection("runtime"));
        assert!(!ctx.has_projection("runtime-marker-extra"));
    }

    #[test]
    fn has_projection_ignores_plain_message_text() {
        let mut ctx = ContextManager::builder(4096).build();
        ctx.push(Message::user(
            "plain text mentions runtime-marker but is not an owned projection".to_string(),
        ));

        assert!(!ctx.has_projection("runtime-marker"));
    }

    #[test]
    fn context_projection_removes_restored_projection_without_transient_state() {
        let mut original = ContextManager::builder(4096)
            .with_system("system".to_string())
            .build();
        original.apply_projections(&[ContextProjection {
            marker: "runtime".to_string(),
            message: Some(Message::user("stale restored projection".to_string())),
        }]);
        let restored_messages = original.messages().to_vec();
        let mut restored = ContextManager::builder(4096).build();
        restored.set_messages(restored_messages);

        restored.apply_projections(&[]);

        assert!(restored.messages().iter().all(|message| {
            message
                .content
                .as_text_ref()
                .is_none_or(|text| !text.contains("stale restored projection"))
        }));
    }

    fn canonical_test_context() -> ContextManager {
        let canonical = CanonicalContext {
            system_prompt: Some("canonical system".to_string()),
            project_rules: Some("canonical rules".to_string()),
            skill_injections: Vec::new(),
            active_skill_names: Vec::new(),
        };
        let mut ctx = ContextManager::builder(1)
            .compressor(SlidingWindowCompressor::new(1))
            .with_system("canonical system".to_string())
            .canonical_context(canonical)
            .build();
        ctx.push(Message::user(
            "enough conversation content to force compression".to_string(),
        ));
        ctx
    }

    fn assert_canonical_checkpoint_matches_context(
        ctx: &ContextManager,
        checkpoint: Option<&CompressionCheckpoint>,
    ) {
        assert_eq!(
            checkpoint.map(|value| value.retained_count),
            Some(ctx.messages().len())
        );
        assert_eq!(
            checkpoint.map(|value| value.token_after),
            Some(ctx.token_estimate())
        );
    }

    #[tokio::test]
    async fn canonical_system_context_is_not_duplicated() -> Result<()> {
        let mut ctx = canonical_test_context();

        let _ = ctx.prepare(None).await?;
        let _ = ctx.prepare(None).await?;

        let system_occurrences = ctx
            .messages()
            .iter()
            .filter_map(|message| message.content.as_text_ref())
            .filter(|content| content.contains("canonical system"))
            .count();
        assert_eq!(system_occurrences, 1);
        Ok(())
    }

    #[tokio::test]
    async fn canonical_system_context_replaces_missing_prompt_beside_other_system_message()
    -> Result<()> {
        let canonical = CanonicalContext {
            system_prompt: Some("canonical system".to_string()),
            project_rules: None,
            skill_injections: Vec::new(),
            active_skill_names: Vec::new(),
        };
        let mut ctx = ContextManager::builder(4096)
            .with_system("different system".to_string())
            .canonical_context(canonical)
            .build();

        let result = ctx.prepare(None).await?;

        assert!(result.messages.iter().any(|message| {
            message.role == Role::System
                && message.content.as_text_ref() == Some("canonical system")
        }));
        Ok(())
    }

    #[tokio::test]
    async fn canonical_prepare_checkpoint_matches_final_context() -> Result<()> {
        let mut ctx = canonical_test_context();

        let result = ctx.prepare(None).await?;

        assert_canonical_checkpoint_matches_context(&ctx, result.checkpoint.as_ref());
        Ok(())
    }

    #[tokio::test]
    async fn canonical_force_compress_checkpoint_matches_final_context() -> Result<()> {
        let mut ctx = canonical_test_context();

        let (_, checkpoint) = ctx.force_compress(1).await?;

        assert_canonical_checkpoint_matches_context(&ctx, checkpoint.as_ref());
        Ok(())
    }

    #[tokio::test]
    async fn canonical_force_compress_with_focus_checkpoint_matches_final_context() -> Result<()> {
        let mut ctx = canonical_test_context();

        let (_, checkpoint) = ctx.force_compress_with_focus("rules", 1).await?;

        assert_canonical_checkpoint_matches_context(&ctx, checkpoint.as_ref());
        Ok(())
    }

    #[tokio::test]
    async fn canonical_force_compress_with_checkpoint_matches_final_context() -> Result<()> {
        let mut ctx = canonical_test_context();
        let compressor = SlidingWindowCompressor::new(1);

        let (_, checkpoint) = ctx.force_compress_with(&compressor).await?;

        assert_canonical_checkpoint_matches_context(&ctx, checkpoint.as_ref());
        Ok(())
    }

    #[tokio::test]
    async fn test_sliding_window_compressor() -> Result<()> {
        println!("=== Example 1: Sliding window compression ===");

        let mut ctx = ContextManager::builder(200)
            .compressor(SlidingWindowCompressor::new(4))
            .build();

        ctx.push(Message::system("You are an assistant.".to_string()));
        for i in 1..=6 {
            ctx.push(Message::user(format!("用户消息 {}", i)));
            ctx.push(Message::assistant(format!("助手回复 {}", i)));
        }

        println!("压缩前消息数：{}", ctx.messages().len());
        let result = ctx.prepare(None).await?;
        let messages = result.messages;
        println!("压缩后消息数：{}", messages.len());
        for m in &messages {
            println!(
                "  [{}] {}",
                m.role.as_str(),
                m.content.as_text_ref().unwrap_or("")
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_protected_messages_keep_relative_position_after_compression() -> Result<()> {
        let mut ctx = ContextManager::builder(10)
            .compressor(SlidingWindowCompressor::new(2))
            .build();
        ctx.add_protected_marker("<skill>".to_string());

        ctx.push(Message::system("system".to_string()));
        ctx.push(Message::user("old user".to_string()));
        ctx.push(Message::assistant("old assistant".to_string()));
        ctx.push(Message::user("<skill> protected".to_string()));
        ctx.push(Message::assistant("recent assistant".to_string()));
        ctx.push(Message::user("latest user".to_string()));

        let (stats, _checkpoint) = ctx.force_compress(2).await?;
        assert!(stats.after_count >= 3);

        let rendered: Vec<(String, String)> = ctx
            .messages()
            .iter()
            .map(|m| {
                (
                    m.role.as_str().to_string(),
                    m.content.as_text_ref().unwrap_or("").to_string(),
                )
            })
            .collect();

        assert_eq!(
            rendered,
            vec![
                ("system".to_string(), "system".to_string()),
                ("user".to_string(), "<skill> protected".to_string()),
                ("assistant".to_string(), "recent assistant".to_string()),
                ("user".to_string(), "latest user".to_string()),
            ]
        );

        Ok(())
    }

    #[test]
    fn test_sanitize_tool_call_pairing_no_tools() {
        let messages = vec![
            Message::system("sys".to_string()),
            Message::user("hello".to_string()),
            Message::assistant("hi".to_string()),
        ];
        let (result, _fixes) = sanitize_tool_call_pairing(&messages);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_sanitize_tool_call_pairing_valid() {
        use echo_core::llm::types::ToolCall;
        let tc = ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: echo_core::llm::types::FunctionCall {
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let messages = vec![
            Message::user("read file".to_string()),
            Message::assistant_with_tools(vec![tc]),
            Message::tool_result(
                "call_1".to_string(),
                "read_file".to_string(),
                "content".to_string(),
            ),
        ];
        let (result, _fixes) = sanitize_tool_call_pairing(&messages);
        assert_eq!(result.len(), 3);
        assert!(result[1].tool_calls.is_some());
    }

    #[test]
    fn test_sanitize_tool_call_pairing_orphaned_tool() {
        // tool result without preceding assistant tool_calls
        let messages = vec![
            Message::user("hello".to_string()),
            Message::tool_result(
                "orphan_1".to_string(),
                "some_tool".to_string(),
                "result".to_string(),
            ),
            Message::assistant("hi".to_string()),
        ];
        let (result, _fixes) = sanitize_tool_call_pairing(&messages);
        // orphaned tool message should be removed
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, Role::User);
        assert_eq!(result[1].role, Role::Assistant);
    }

    #[test]
    fn test_sanitize_tool_call_pairing_dangling_calls() {
        use echo_core::llm::types::ToolCall;
        // assistant has tool_calls but no tool results (compression removed them)
        // When ALL tool_calls are orphaned, we null out the tool_calls field
        // rather than adding placeholder results.
        let tc = ToolCall {
            id: "call_missing".to_string(),
            call_type: "function".to_string(),
            function: echo_core::llm::types::FunctionCall {
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let messages = vec![
            Message::user("read file".to_string()),
            Message::assistant_with_tools(vec![tc]),
            Message::user("next question".to_string()),
        ];
        let (result, _fixes) = sanitize_tool_call_pairing(&messages);
        // assistant's tool_calls should be nulled out → 3 messages (no placeholder)
        assert_eq!(result.len(), 3);
        assert!(
            result[1].tool_calls.is_none(),
            "orphaned tool_calls should be removed"
        );
    }

    #[test]
    fn test_sanitize_tool_call_pairing_mixed() {
        use echo_core::llm::types::ToolCall;
        // assistant has 2 tool_calls, but only 1 result exists
        let tc1 = ToolCall {
            id: "call_present".to_string(),
            call_type: "function".to_string(),
            function: echo_core::llm::types::FunctionCall {
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let tc2 = ToolCall {
            id: "call_missing".to_string(),
            call_type: "function".to_string(),
            function: echo_core::llm::types::FunctionCall {
                name: "write_file".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let messages = vec![
            Message::user("do stuff".to_string()),
            Message::assistant_with_tools(vec![tc1, tc2]),
            Message::tool_result(
                "call_present".to_string(),
                "read_file".to_string(),
                "content".to_string(),
            ),
            // call_missing result was removed by compression
            Message::user("next".to_string()),
        ];
        let (result, _fixes) = sanitize_tool_call_pairing(&messages);
        // Should have: user, assistant, tool(present), tool(placeholder), user
        assert_eq!(result.len(), 5);
        assert_eq!(result[2].tool_call_id.as_deref(), Some("call_present"));
        assert_eq!(result[3].role, Role::Tool);
        assert_eq!(result[3].tool_call_id.as_deref(), Some("call_missing"));
        assert!(
            result[3]
                .content
                .as_text_ref()
                .unwrap()
                .contains("unavailable")
        );
    }

    // ── L3 Memory Promotion tests ────────────────────────────────────

    /// A test promoter that records how many times it was called and
    /// how many evicted messages it received.
    struct TestPromoter {
        call_count: Arc<std::sync::atomic::AtomicUsize>,
        total_evicted: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl TestPromoter {
        fn new() -> (
            Self,
            Arc<std::sync::atomic::AtomicUsize>,
            Arc<std::sync::atomic::AtomicUsize>,
        ) {
            let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let total_evicted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            (
                Self {
                    call_count: call_count.clone(),
                    total_evicted: total_evicted.clone(),
                },
                call_count,
                total_evicted,
            )
        }
    }

    impl MemoryPromoter for TestPromoter {
        fn promote(&self, evicted: &[Message]) -> futures::future::BoxFuture<'_, ()> {
            self.call_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.total_evicted
                .fetch_add(evicted.len(), std::sync::atomic::Ordering::Relaxed);
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn test_memory_promoter_called_on_compression() -> Result<()> {
        let (promoter, call_count, total_evicted) = TestPromoter::new();

        let mut ctx = ContextManager::builder(100) // Very low token limit to trigger compression
            .compressor(SlidingWindowCompressor::new(4))
            .memory_promoter(Arc::new(promoter))
            .build();

        // Push enough messages to trigger compression
        ctx.push(Message::system("You are a helper.".to_string()));
        for i in 1..=10 {
            ctx.push(Message::user(format!(
                "Question number {} about various topics",
                i
            )));
            ctx.push(Message::assistant(format!(
                "Here is a detailed answer to question {} with some explanation",
                i
            )));
        }

        let _result = ctx.prepare(None).await?;

        let calls = call_count.load(std::sync::atomic::Ordering::Relaxed);
        let evicted = total_evicted.load(std::sync::atomic::Ordering::Relaxed);

        assert!(
            calls > 0,
            "Memory promoter should have been called at least once"
        );
        assert!(
            evicted > 0,
            "Promoter should have received evicted messages, got {}",
            evicted
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_compression_reduces_tokens_by_30_percent() -> Result<()> {
        let mut ctx = ContextManager::builder(200)
            .compressor(SlidingWindowCompressor::new(6))
            .build();

        ctx.push(Message::system("You are a helpful assistant.".to_string()));

        // Build a conversation with large tool outputs
        for i in 1..=20 {
            ctx.push(Message::user(format!("Question {}", i)));
            ctx.push(Message::assistant(format!(
                "Let me help you with question {}. I'll use some tools.",
                i
            )));
            // Simulate large tool output
            ctx.push(Message::user(format!("[Tool result: {}]", "x".repeat(500))));
        }

        let tokens_before = ctx.token_estimate();
        let _result = ctx.prepare(None).await?;
        let tokens_after = ctx.token_estimate();

        if tokens_before > 200 {
            // Only assert if we actually had enough tokens to compress
            let reduction = 1.0 - (tokens_after as f64 / tokens_before as f64);
            assert!(
                reduction > 0.3,
                "Token reduction should be >30%: before={}, after={}, reduction={:.1}%",
                tokens_before,
                tokens_after,
                reduction * 100.0
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_memory_promoter_not_called_without_compression() -> Result<()> {
        let (promoter, call_count, _) = TestPromoter::new();

        let mut ctx = ContextManager::builder(1_000_000) // Very high limit — no compression
            .memory_promoter(Arc::new(promoter))
            .build();

        ctx.push(Message::system("sys".to_string()));
        ctx.push(Message::user("hello".to_string()));
        ctx.push(Message::assistant("hi".to_string()));

        let _result = ctx.prepare(None).await?;

        let calls = call_count.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            calls, 0,
            "Promoter should NOT be called when no compression occurs"
        );
        Ok(())
    }
}
