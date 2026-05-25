//! Context compression
//!
//! Maintains conversation history and automatically compresses when tokens exceed the limit, managed by [`ContextManager`].
//!
//! Built-in compression strategies (all implement the [`ContextCompressor`] trait):
//! - [`compressor::SlidingWindowCompressor`]: Sliding window, discards the oldest N messages
//! - [`compressor::SummaryCompressor`]: LLM summarization, compresses old messages into a system summary message
//! - [`compressor::HybridCompressor`]: Multi-strategy pipeline chaining

pub mod compressor;

// Re-export from echo_core for backward compatibility
pub use echo_core::compression::{CompressionInput, CompressionOutput, ContextCompressor};

use crate::compression::compressor::SlidingWindowCompressor;
use echo_core::error::Result;
use echo_core::llm::types::{Message, MessageContent, Role};
use echo_core::budget::TokenBudget;
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use std::sync::Arc;

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

    /// Remove the compressor, reverting to unlimited mode
    pub fn remove_compressor(&mut self) {
        self.compressor = None;
    }

    /// Whether a compressor is configured
    pub fn has_compressor(&self) -> bool {
        self.compressor.is_some()
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
            let input = CompressionInput {
                messages: compressible,
                token_limit: self.token_limit,
                current_query: None,
            };
            compressor.compress(input).await?
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
        Ok(ForceCompressStats {
            before_count,
            after_count: self.messages.len(),
            evicted,
            before_tokens,
            after_tokens: self.token_estimate(),
        })
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
        Ok(ForceCompressStats {
            before_count,
            after_count: self.messages.len(),
            evicted,
            before_tokens,
            after_tokens: self.token_estimate(),
        })
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
        let estimated_tokens = Self::estimate_tokens(&self.messages, &*self.tokenizer);

        let needs_compression = if let Some(ref budget) = self.budget {
            // Budget-aware check: use percentage-based allocation
            let system_tokens = 0; // system prompt tokens already counted in messages
            let tool_tokens = 0;   // tool defs not in messages
            let allocation = budget.allocate(system_tokens, tool_tokens, estimated_tokens);
            allocation.needs_compression()
        } else {
            estimated_tokens > self.token_limit
        };

        let compressed = if let Some(compressor) = &self.compressor && needs_compression {
            let before_count = self.messages.len();
            let before_tokens = self.token_estimate();

            // Compute effective token limit for compression
            let effective_limit = if let Some(ref budget) = self.budget {
                let allocation = budget.allocate(0, 0, estimated_tokens);
                (estimated_tokens.saturating_sub(allocation.conversation_excess)).max(self.token_limit / 2)
            } else {
                self.token_limit
            };

            let (compressible, protected) = self.split_protected(self.messages.clone());

            let output = compressor
                .compress(CompressionInput {
                    messages: compressible,
                    token_limit: effective_limit,
                    current_query: current_query.map(String::from),
                })
                .await?;

            let evicted = output.evicted.len();
            self.messages = Self::merge_protected(output.messages, protected);

            Some(ForceCompressStats {
                before_count,
                after_count: self.messages.len(),
                evicted,
                before_tokens,
                after_tokens: self.token_estimate(),
            })
        } else {
            None
        };

        Ok(PrepareResult {
            messages: self.messages.clone(),
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

/// Builder for `ContextManager`
pub struct ContextManagerBuilder {
    token_limit: usize,
    compressor: Option<Box<dyn ContextCompressor>>,
    initial_messages: Vec<Message>,
    tokenizer: Option<Arc<dyn Tokenizer>>,
    max_messages: Option<usize>,
    budget: Option<TokenBudget>,
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
}
