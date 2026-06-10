//! Context compression
//!
//! Maintains conversation history and automatically compresses when tokens exceed the limit, managed by [`ContextManager`].
//!
//! Built-in compression strategies (all implement the [`ContextCompressor`] trait):
//! - [`compressor::SlidingWindowCompressor`]: Sliding window, discards the oldest N messages
//! - [`compressor::SummaryCompressor`]: LLM summarization, compresses old messages into a system summary message
//! - [`compressor::HybridCompressor`]: Multi-strategy pipeline chaining

pub mod compressor;
pub mod horizon;
pub mod levels;

// Re-export from echo_core for backward compatibility
pub use echo_core::compression::{CompressionInput, CompressionOutput, ContextCompressor};

use crate::compression::compressor::SlidingWindowCompressor;
use echo_core::budget::TokenBudget;
use echo_core::error::Result;
use echo_core::llm::types::{Message, MessageContent, Role};
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use std::sync::Arc;

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
        self.total_tokens_before.saturating_sub(self.total_tokens_after)
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
/// # Hybrid pipeline example
///
/// ```rust,no_run
/// use echo_core::error::Result;
/// use echo_core::llm::LlmClient;
/// use echo_state::compression::compressor::{
///     HybridCompressor, SlidingWindowCompressor, SummaryCompressor,
/// };
/// use echo_state::compression::{ContextCompressor, ContextManager};
/// use std::sync::Arc;
///
/// # async fn example(llm: Arc<dyn LlmClient>) -> Result<()> {
/// let compressor = HybridCompressor::builder()
///     .stage(SlidingWindowCompressor::new(30))
///     .stage(SummaryCompressor::new(llm, 8))
///     .build();
///
/// let mut ctx = ContextManager::builder(8192)
///     .compressor(compressor)
///     .build();
/// # Ok(())
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
    /// Hard message count cap. When exceeded, triggers sliding window degradation to prevent OOM.
    /// Default 200 messages.
    max_messages: usize,
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
        }
    }

    /// Append a message to the context buffer.
    ///
    /// When the message count exceeds the `max_messages` hard cap, automatically applies sliding window degradation:
    /// preserves system messages and recent messages, discards the earliest conversation messages in the middle.
    /// This is the last line of defense; even if no compressor is configured or compression fails, OOM will not occur.
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);

        // Hard cap degradation: apply sliding window when exceeding max_messages
        if self.messages.len() > self.max_messages {
            self.apply_hard_cap();
        }
    }

    /// Apply hard message cap: preserve system messages, protected messages, and recent messages; discard the earliest in between.
    fn apply_hard_cap(&mut self) {
        let target = self.max_messages;
        if self.messages.len() <= target {
            return;
        }

        // Identify protected messages (should not be deleted)
        let mut protected_indices: Vec<usize> = Vec::new();
        for (i, msg) in self.messages.iter().enumerate() {
            if self.is_protected(msg) {
                protected_indices.push(i);
            }
        }

        // Find the position of the first non-system message
        let first_non_system = self
            .messages
            .iter()
            .position(|m| m.role != Role::System)
            .unwrap_or(0);

        // Calculate how many non-protected messages need to be deleted
        let excess = self.messages.len() - target;
        let mut to_remove = Vec::new();
        let mut removed = 0;
        for i in first_non_system..self.messages.len() {
            if removed >= excess {
                break;
            }
            // Skip protected messages
            if protected_indices.contains(&i) {
                continue;
            }
            to_remove.push(i);
            removed += 1;
        }

        if to_remove.is_empty() {
            return;
        }

        tracing::warn!(
            total = self.messages.len(),
            cap = target,
            evicted = to_remove.len(),
            "Message count exceeded hard cap, applying sliding window degradation (preserving protected messages)"
        );

        // Remove from back to front to avoid index shifting
        for &i in to_remove.iter().rev() {
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

    /// Check if a message is protected from compression.
    fn is_protected(&self, message: &Message) -> bool {
        if self.protected_markers.is_empty() {
            return false;
        }
        if let Some(content) = message.content.as_text() {
            self.protected_markers.iter().any(|m| content.contains(m))
        } else {
            false
        }
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

    /// Force-compress the context, regardless of whether the current token count exceeds the limit.
    ///
    /// - If a compressor is configured, use it;
    /// - Otherwise, temporarily use `SlidingWindowCompressor::new(fallback_window)`.
    ///
    /// Protected messages are excluded from compression and preserved.
    pub async fn force_compress(&mut self, fallback_window: usize) -> Result<ForceCompressStats> {
        let before_count = self.messages.len();
        let before_tokens = self.token_estimate();

        let (compressible, protected) = self.split_protected(self.messages.clone());

        let output = if let Some(compressor) = &self.compressor {
            compressor
                .compress(CompressionInput {
                    messages: compressible,
                    token_limit: self.token_limit,
                    current_query: None,
                })
                .await?
        } else {
            SlidingWindowCompressor::new(fallback_window)
                .compress(CompressionInput {
                    messages: compressible,
                    token_limit: self.token_limit,
                    current_query: None,
                })
                .await?
        };

        let evicted = output.evicted.len();
        self.messages = Self::merge_protected(output.messages, protected);
        let stats = ForceCompressStats {
            before_count,
            after_count: self.messages.len(),
            evicted,
            before_tokens,
            after_tokens: self.token_estimate(),
        };
        let name = if self.compressor.is_some() {
            self.compressor.as_ref().map(|c| c.name()).unwrap_or("unknown")
        } else {
            "SlidingWindow(fallback)"
        };
        self.metrics.record(&stats, name);
        Ok(stats)
    }

    /// Force-compress using a **specific compressor**, without affecting the currently installed compressor config.
    ///
    /// Suitable for temporary strategy overrides like `/compress sliding 10`.
    pub async fn force_compress_with(
        &mut self,
        compressor: &dyn ContextCompressor,
    ) -> Result<ForceCompressStats> {
        let before_count = self.messages.len();
        let before_tokens = self.token_estimate();

        let (compressible, protected) = self.split_protected(self.messages.clone());

        let output = compressor
            .compress(CompressionInput {
                messages: compressible,
                token_limit: self.token_limit,
                current_query: None,
            })
            .await?;

        let evicted = output.evicted.len();
        self.messages = Self::merge_protected(output.messages, protected);
        let stats = ForceCompressStats {
            before_count,
            after_count: self.messages.len(),
            evicted,
            before_tokens,
            after_tokens: self.token_estimate(),
        };
        self.metrics.record(&stats, compressor.name());
        Ok(stats)
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
            };
            match horizon_compressor.compress(horizon_input).await {
                Ok(output) => {
                    let compacted = before.saturating_sub(output.messages.len());
                    if compacted > 0 {
                        tracing::debug!(
                            before_messages = before,
                            after_messages = output.messages.len(),
                            evicted = output.evicted.len(),
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

        let needs_compression = if let Some(ref budget) = self.budget {
            // Budget-aware check: use percentage-based allocation
            let system_tokens = 0; // system prompt tokens already counted in messages
            let tool_tokens = 0; // tool defs not in messages
            let allocation = budget.allocate(system_tokens, tool_tokens, estimated_tokens);
            allocation.needs_compression()
        } else {
            estimated_tokens > self.token_limit
        };

        let compressed = if let Some(compressor) = &self.compressor
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

            // Compute effective token limit for compression
            let effective_limit = if let Some(ref budget) = self.budget {
                let allocation = budget.allocate(0, 0, estimated_tokens);
                (estimated_tokens.saturating_sub(allocation.conversation_excess))
                    .max(self.token_limit / 2)
            } else {
                self.token_limit
            };

            let owned = std::mem::take(&mut self.messages);
            let (compressible, protected) = self.split_protected(owned);

            let compress_result = compressor
                .compress(CompressionInput {
                    messages: compressible,
                    token_limit: effective_limit,
                    current_query: current_query.map(String::from),
                })
                .await;

            match compress_result {
                Ok(output) => {
                    let evicted_messages = output.evicted;
                    let evicted = evicted_messages.len();
                    self.messages = Self::merge_protected(output.messages, protected);

                    // ── L3 Memory Promotion ──
                    // If a memory promoter is configured, pass evicted messages
                    // so key facts can be extracted and stored for later recall.
                    if let Some(ref promoter) = self.memory_promoter {
                        if !evicted_messages.is_empty() {
                            promoter.promote(&evicted_messages).await;
                        }
                    }

                    let stats = ForceCompressStats {
                        before_count,
                        after_count: self.messages.len(),
                        evicted,
                        before_tokens,
                        after_tokens: self.token_estimate(),
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

                    Some(stats)
                }
                Err(e) => {
                    // Compression failed — restore protected messages at minimum.
                    self.messages = Self::merge_protected(vec![], protected);
                    tracing::warn!(error = %e, "Compression failed, messages may be partially lost");
                    return Err(e);
                }
            }
        } else {
            None
        };

        // Always sanitize tool_calls → tool_result pairing before sending to LLM.
        // Even without compression, session resume or manual manipulation can
        // produce invalid sequences.
        let messages = sanitize_tool_call_pairing(&self.messages);

        Ok(PrepareResult {
            messages,
            compressed,
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
fn sanitize_tool_call_pairing(messages: &[Message]) -> Vec<Message> {
    use std::collections::{HashMap, HashSet};

    if messages.is_empty() {
        return vec![];
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
        return messages.to_vec();
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
                    id,
                    "unknown".to_string(),
                    "[Result unavailable — tool result was removed during context compression]".to_string(),
                ));
            }
        }
    }

    result
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
}

impl ContextManagerBuilder {
    /// Set the compression strategy (optional). Supports any type implementing `ContextCompressor`,
    /// including `SlidingWindowCompressor`, `SummaryCompressor`, and `HybridCompressor`.
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

    /// Set the hard message count cap (default 200).
    ///
    /// When exceeded, automatically applies sliding window degradation, preserving system messages and recent messages.
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

    pub fn build(self) -> ContextManager {
        ContextManager {
            messages: self.initial_messages,
            compressor: self.compressor,
            token_limit: self.token_limit,
            tokenizer: self
                .tokenizer
                .unwrap_or_else(|| Arc::new(HeuristicTokenizer)),
            protected_markers: Vec::new(),
            max_messages: self.max_messages.unwrap_or(200),
            budget: self.budget,
            metrics: CompressionMetrics::new(),
            visibility_horizon: self.visibility_horizon,
            memory_promoter: self.memory_promoter,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::compressor::SlidingWindowCompressor;
    use echo_core::error::Result;

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

        let messages = ctx.force_compress(2).await?;
        assert!(messages.after_count >= 3);

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
        let result = sanitize_tool_call_pairing(&messages);
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
            Message::tool_result("call_1".to_string(), "read_file".to_string(), "content".to_string()),
        ];
        let result = sanitize_tool_call_pairing(&messages);
        assert_eq!(result.len(), 3);
        assert!(result[1].tool_calls.is_some());
    }

    #[test]
    fn test_sanitize_tool_call_pairing_orphaned_tool() {
        // tool result without preceding assistant tool_calls
        let messages = vec![
            Message::user("hello".to_string()),
            Message::tool_result("orphan_1".to_string(), "some_tool".to_string(), "result".to_string()),
            Message::assistant("hi".to_string()),
        ];
        let result = sanitize_tool_call_pairing(&messages);
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
        let result = sanitize_tool_call_pairing(&messages);
        // assistant's tool_calls should be nulled out → 3 messages (no placeholder)
        assert_eq!(result.len(), 3);
        assert!(result[1].tool_calls.is_none(), "orphaned tool_calls should be removed");
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
            Message::tool_result("call_present".to_string(), "read_file".to_string(), "content".to_string()),
            // call_missing result was removed by compression
            Message::user("next".to_string()),
        ];
        let result = sanitize_tool_call_pairing(&messages);
        // Should have: user, assistant, tool(present), tool(placeholder), user
        assert_eq!(result.len(), 5);
        assert_eq!(result[2].tool_call_id.as_deref(), Some("call_present"));
        assert_eq!(result[3].role, Role::Tool);
        assert_eq!(result[3].tool_call_id.as_deref(), Some("call_missing"));
        assert!(result[3].content.as_text_ref().unwrap().contains("unavailable"));
    }

    // ── L3 Memory Promotion tests ────────────────────────────────────

    /// A test promoter that records how many times it was called and
    /// how many evicted messages it received.
    struct TestPromoter {
        call_count: Arc<std::sync::atomic::AtomicUsize>,
        total_evicted: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl TestPromoter {
        fn new() -> (Self, Arc<std::sync::atomic::AtomicUsize>, Arc<std::sync::atomic::AtomicUsize>) {
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
            ctx.push(Message::user(format!("Question number {} about various topics", i)));
            ctx.push(Message::assistant(format!(
                "Here is a detailed answer to question {} with some explanation",
                i
            )));
        }

        let _result = ctx.prepare(None).await?;

        let calls = call_count.load(std::sync::atomic::Ordering::Relaxed);
        let evicted = total_evicted.load(std::sync::atomic::Ordering::Relaxed);

        assert!(calls > 0, "Memory promoter should have been called at least once");
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
            ctx.push(Message::user(format!(
                "[Tool result: {}]",
                "x".repeat(500)
            )));
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
