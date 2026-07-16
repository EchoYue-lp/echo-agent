//! Multi-level adaptive compression.
//!
//! Levels from cheapest to most expensive:
//! L1 Snip: Remove large tool outputs (>N tokens)
//! L2 Micro: Truncate tool outputs to keep first/last N lines
//! L3 Context Collapse: Summarize older message groups (non-LLM, rule-based)
//! L4 Auto Compact: Full LLM summarization (handled externally)
//! L5 Reactive: Emergency — keep only system prompt + last 3 messages

use crate::compression::CompressionCheckpoint;
use echo_core::llm::LlmClient;
use echo_core::llm::types::{Message, MessageContent, Role};
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

/// Truncate a string at a byte boundary without splitting multi-byte UTF-8 characters.
///
/// Returns the longest prefix of `s` whose byte length is ≤ `max_bytes`
/// and ends on a valid char boundary.
fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    // Walk backwards from max_bytes to find the nearest char boundary
    let end = s
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .take_while(|&end| end <= max_bytes)
        .last()
        .unwrap_or(0);
    &s[..end]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveCompressionConfig {
    /// Token threshold to trigger L1 (snip large outputs)
    pub l1_snip_threshold_tokens: usize,
    /// Max tokens per tool output before snipping
    pub l1_max_output_tokens: usize,
    /// Whether L1 also folds consecutive tool results into summaries.
    /// When true, runs after snipping to collapse long runs of tool messages.
    #[serde(default = "default_true")]
    pub l1_fold_consecutive_tools: bool,
    /// When folding consecutive tool results, keep the latest N and collapse older ones.
    #[serde(default = "default_l1_fold_keep_latest")]
    pub l1_fold_keep_latest: usize,
    /// Token threshold to trigger L2 (truncate)
    pub l2_micro_threshold_tokens: usize,
    /// Lines to keep from start/end of truncated outputs
    pub l2_keep_lines: usize,
    /// Token threshold to trigger L3 (context collapse)
    pub l3_collapse_threshold_tokens: usize,
    /// Number of recent messages to keep intact during collapse
    pub l3_keep_recent: usize,
    /// Token threshold to trigger L4 (LLM summarization)
    pub l4_compact_threshold_tokens: usize,
    /// Number of recent messages to keep during LLM compact
    pub l4_keep_recent: usize,
}

fn default_true() -> bool {
    true
}
fn default_l1_fold_keep_latest() -> usize {
    2
}

impl Default for AdaptiveCompressionConfig {
    fn default() -> Self {
        Self {
            l1_snip_threshold_tokens: 80_000,
            l1_max_output_tokens: 4_000,
            l1_fold_consecutive_tools: true,
            l1_fold_keep_latest: 2,
            l2_micro_threshold_tokens: 100_000,
            l2_keep_lines: 50,
            l3_collapse_threshold_tokens: 120_000,
            l3_keep_recent: 10,
            l4_compact_threshold_tokens: 150_000,
            l4_keep_recent: 6,
        }
    }
}

/// Auto-tune compression thresholds based on the model's context window size.
///
/// Thresholds are set as percentages of the context window:
/// - L1 (Snip): 60%
/// - L2 (Micro): 75%
/// - L3 (Collapse): 85%
/// - L4 (Compact): 90%
///
/// L5 (Reactive) is not tuned — it triggers at 2× the L4 threshold regardless.
///
/// # Example
///
/// ```rust
/// use echo_state::compression::levels::{AdaptiveCompressionConfig, tune_for_model};
///
/// let mut config = AdaptiveCompressionConfig::default();
/// tune_for_model(&mut config, 200_000); // Claude's context window
/// // Now L1=120K, L2=150K, L3=170K, L4=180K
/// ```
pub fn tune_for_model(config: &mut AdaptiveCompressionConfig, context_window: usize) {
    let w = context_window;
    config.l1_snip_threshold_tokens = w * 60 / 100;
    config.l2_micro_threshold_tokens = w * 75 / 100;
    config.l3_collapse_threshold_tokens = w * 85 / 100;
    config.l4_compact_threshold_tokens = w * 90 / 100;
    // L5 triggers at tokens > target * 2, so it's automatically adapted
}

/// Result of adaptive compression showing which levels were applied.
#[derive(Debug, Clone)]
pub struct AdaptiveCompressionResult {
    pub levels_applied: Vec<String>,
    pub tokens_before: usize,
    pub tokens_after: usize,
}

/// Adaptive multi-level compressor.
///
/// Applies compression levels in order from cheapest to most expensive,
/// stopping when the token count is below the target threshold.
///
/// L4 (LLM summarization) is only available when an LLM client is configured
/// via [`with_llm()`](Self::with_llm). Without it, L4 is skipped and L5
/// (emergency) activates when needed.
pub struct AdaptiveCompressor {
    config: AdaptiveCompressionConfig,
    tokenizer: HeuristicTokenizer,
    /// Optional LLM client for L4 auto-compact.
    llm: Option<Arc<dyn LlmClient>>,
}

impl AdaptiveCompressor {
    pub fn new(config: AdaptiveCompressionConfig) -> Self {
        Self {
            config,
            tokenizer: HeuristicTokenizer,
            llm: None,
        }
    }

    /// Set an LLM client to enable L4 auto-compact (full LLM summarization).
    ///
    /// Without an LLM client, the adaptive pipeline skips L4 and falls
    /// through to L5 (emergency) when tokens remain above threshold after L3.
    pub fn with_llm(mut self, llm: Arc<dyn LlmClient>) -> Self {
        self.llm = Some(llm);
        self
    }

    /// Compress messages adaptively in place.
    ///
    /// This is the low-level API that mutates `messages` directly.
    /// For integration with [`ContextManager`](super::ContextManager),
    /// use the [`ContextCompressor`](super::ContextCompressor) trait implementation instead.
    pub fn compress_in_place(
        &self,
        messages: &mut Vec<Message>,
        current_tokens: usize,
        target_tokens: usize,
    ) -> AdaptiveCompressionResult {
        let (result, _evicted) =
            self.compress_with_evicted(messages, current_tokens, target_tokens);
        result
    }

    /// Internal: compress and track evicted messages.
    fn compress_with_evicted(
        &self,
        messages: &mut Vec<Message>,
        current_tokens: usize,
        target_tokens: usize,
    ) -> (AdaptiveCompressionResult, Vec<Message>) {
        let tokens_before = current_tokens;
        let mut levels_applied = Vec::new();
        let mut tokens = current_tokens;
        let mut all_evicted = Vec::new();

        // L1: Snip large tool outputs
        if tokens > self.config.l1_snip_threshold_tokens && tokens > target_tokens {
            let snipped = self.apply_l1_snip(messages);
            tokens = tokens.saturating_sub(snipped);
            if snipped > 0 {
                levels_applied.push("L1:Snip".to_string());
            }

            // L1 also folds consecutive tool results (runs after snipping)
            if self.config.l1_fold_consecutive_tools {
                let (folded_tokens, folded_evicted) = self.apply_l1_fold_tools(messages);
                tokens = tokens.saturating_sub(folded_tokens);
                if !folded_evicted.is_empty() {
                    if !levels_applied.contains(&"L1:Fold".to_string()) {
                        levels_applied.push("L1:Fold".to_string());
                    }
                    all_evicted.extend(folded_evicted);
                }
            }
        }

        // L2: Micro — truncate tool outputs
        if tokens > self.config.l2_micro_threshold_tokens && tokens > target_tokens {
            let truncated = self.apply_l2_micro(messages);
            tokens = tokens.saturating_sub(truncated);
            if truncated > 0 {
                levels_applied.push("L2:Micro".to_string());
            }
        }

        // L3: Context Collapse — remove older messages, keep recent
        if tokens > self.config.l3_collapse_threshold_tokens && tokens > target_tokens {
            let (collapsed, evicted) = self.apply_l3_collapse(messages);
            tokens = tokens.saturating_sub(collapsed);
            if collapsed > 0 {
                levels_applied.push("L3:Collapse".to_string());
                all_evicted.extend(evicted);
            }
        }

        // L5: Reactive — emergency (L4 requires LLM, handled externally)
        if tokens > target_tokens * 2 && tokens > self.config.l4_compact_threshold_tokens {
            let (saved, evicted) = self.apply_l5_reactive(messages);
            tokens = tokens.saturating_sub(saved);
            if saved > 0 {
                levels_applied.push("L5:Reactive".to_string());
                all_evicted.extend(evicted);
            }
        }

        (
            AdaptiveCompressionResult {
                levels_applied,
                tokens_before,
                tokens_after: tokens,
            },
            all_evicted,
        )
    }

    /// Async compression pipeline including L4 (LLM summarization).
    ///
    /// Runs L1 → L2 → L3 → L4 (if LLM configured) → L5.
    /// This is the full pipeline used by the [`ContextCompressor`] trait.
    async fn compress_async(
        &self,
        messages: &mut Vec<Message>,
        current_tokens: usize,
        target_tokens: usize,
        focus: Option<&str>,
    ) -> (AdaptiveCompressionResult, Vec<Message>, Option<String>) {
        let tokens_before = current_tokens;
        let mut levels_applied = Vec::new();
        let mut tokens = current_tokens;
        let mut all_evicted = Vec::new();
        let mut l4_summary: Option<String> = None;

        // L1: Snip large tool outputs + fold consecutive tools
        if tokens > self.config.l1_snip_threshold_tokens && tokens > target_tokens {
            let snipped = self.apply_l1_snip(messages);
            tokens = tokens.saturating_sub(snipped);
            if snipped > 0 {
                levels_applied.push("L1:Snip".to_string());
            }
            if self.config.l1_fold_consecutive_tools {
                let (folded_tokens, folded_evicted) = self.apply_l1_fold_tools(messages);
                tokens = tokens.saturating_sub(folded_tokens);
                if !folded_evicted.is_empty() {
                    if !levels_applied.contains(&"L1:Fold".to_string()) {
                        levels_applied.push("L1:Fold".to_string());
                    }
                    all_evicted.extend(folded_evicted);
                }
            }
        }

        // L2: Micro — truncate tool outputs
        if tokens > self.config.l2_micro_threshold_tokens && tokens > target_tokens {
            let truncated = self.apply_l2_micro(messages);
            tokens = tokens.saturating_sub(truncated);
            if truncated > 0 {
                levels_applied.push("L2:Micro".to_string());
            }
        }

        // L3: Context Collapse — remove older messages, keep recent
        if tokens > self.config.l3_collapse_threshold_tokens && tokens > target_tokens {
            let (collapsed, evicted) = self.apply_l3_collapse(messages);
            tokens = tokens.saturating_sub(collapsed);
            if collapsed > 0 {
                levels_applied.push("L3:Collapse".to_string());
                all_evicted.extend(evicted);
            }
        }

        // L4: Auto Compact — LLM summarization (only if LLM client is configured)
        if tokens > self.config.l4_compact_threshold_tokens
            && tokens > target_tokens
            && let Some(llm) = &self.llm
        {
            let (saved, evicted, summary) =
                self.apply_l4_compact(messages, llm.as_ref(), focus).await;
            tokens = tokens.saturating_sub(saved);
            if saved > 0 {
                levels_applied.push("L4:Compact".to_string());
                all_evicted.extend(evicted);
                l4_summary = summary;
            }
        }

        // L5: Reactive — emergency
        if tokens > target_tokens * 2 && tokens > self.config.l4_compact_threshold_tokens {
            let (saved, evicted) = self.apply_l5_reactive(messages);
            tokens = tokens.saturating_sub(saved);
            if saved > 0 {
                levels_applied.push("L5:Reactive".to_string());
                all_evicted.extend(evicted);
            }
        }

        (
            AdaptiveCompressionResult {
                levels_applied,
                tokens_before,
                tokens_after: tokens,
            },
            all_evicted,
            l4_summary,
        )
    }

    /// L1: Remove tool outputs that exceed max_output_tokens.
    fn apply_l1_snip(&self, messages: &mut [Message]) -> usize {
        let mut saved = 0;
        let max_tokens = self.config.l1_max_output_tokens;

        for msg in messages.iter_mut() {
            if msg.role != Role::Tool {
                continue;
            }
            let text = msg.content.as_text_ref().unwrap_or("");
            let tokens = self.tokenizer.count_tokens(text);
            if tokens > max_tokens {
                let char_limit = max_tokens * 4; // ~4 chars per token
                let truncated = format!(
                    "{}\n...[output truncated: {} tokens, kept first {}]...",
                    safe_truncate(text, char_limit),
                    tokens,
                    max_tokens
                );
                saved += tokens - max_tokens;
                msg.content = MessageContent::Text(truncated);
            }
        }
        saved
    }

    /// L1 Fold: Collapse consecutive tool result messages, keeping only the latest N per run.
    /// Returns (tokens_saved, evicted_messages).
    fn apply_l1_fold_tools(&self, messages: &mut Vec<Message>) -> (usize, Vec<Message>) {
        let keep = self.config.l1_fold_keep_latest;
        let mut saved = 0;
        let mut evicted = Vec::new();
        let mut i = 0;

        while i < messages.len() {
            if messages[i].role != Role::Tool {
                i += 1;
                continue;
            }
            // Find the run of consecutive tool messages
            let start = i;
            while i < messages.len() && messages[i].role == Role::Tool {
                i += 1;
            }
            let count = i - start;
            if count > keep {
                let to_remove = count - keep;
                // Collect evicted messages and their tokens
                for msg in &messages[start..start + to_remove] {
                    let tokens = self
                        .tokenizer
                        .count_tokens(msg.content.as_text_ref().unwrap_or(""));
                    saved += tokens;
                    evicted.push(msg.clone());
                }
                // Replace removed messages with a fold summary
                let fold_msg = Message::user(format!(
                    "[L1 fold: {to_remove} consecutive tool results collapsed]"
                ));
                messages.drain(start..start + to_remove);
                messages.insert(start, fold_msg);
                // Adjust index: removed `to_remove`, inserted 1
                i = start + 1 + keep;
            }
        }

        (saved, evicted)
    }

    /// L2: Truncate tool outputs to keep first/last N lines.
    fn apply_l2_micro(&self, messages: &mut [Message]) -> usize {
        let mut saved = 0;
        let keep = self.config.l2_keep_lines;

        for msg in messages.iter_mut() {
            if msg.role != Role::Tool {
                continue;
            }
            let text = msg.content.as_text_ref().unwrap_or("");
            let lines: Vec<&str> = text.lines().collect();
            if lines.len() > keep * 2 {
                let head: String = lines[..keep].join("\n");
                let tail: String = lines[lines.len() - keep..].join("\n");
                let new_text = format!(
                    "{}\n...[{} lines truncated]...\n{}",
                    head,
                    lines.len() - keep * 2,
                    tail
                );
                let old_tokens = self.tokenizer.count_tokens(text);
                let new_tokens = self.tokenizer.count_tokens(&new_text);
                saved += old_tokens.saturating_sub(new_tokens);
                msg.content = MessageContent::Text(new_text);
            }
        }
        saved
    }

    /// L3: Remove older messages, keeping only recent N.
    /// Returns (tokens_saved, evicted_messages).
    fn apply_l3_collapse(&self, messages: &mut Vec<Message>) -> (usize, Vec<Message>) {
        let keep = self.config.l3_keep_recent;
        if messages.len() <= keep + 1 {
            return (0, vec![]);
        }

        // Keep system messages + last N messages
        let system_msgs: Vec<Message> = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .cloned()
            .collect();
        let recent: Vec<Message> = messages.iter().rev().take(keep).rev().cloned().collect();

        let removed: Vec<Message> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .take(messages.len().saturating_sub(keep + system_msgs.len()))
            .cloned()
            .collect();
        let saved: usize = removed
            .iter()
            .map(|m| {
                self.tokenizer
                    .count_tokens(m.content.as_text_ref().unwrap_or(""))
            })
            .sum();

        // Add a summary message about what was removed
        let summary = format!(
            "[Context compressed: {} older messages removed to save space]",
            removed.len()
        );
        let mut new_messages = system_msgs;
        new_messages.push(Message::system(summary));
        new_messages.extend(recent);
        *messages = new_messages;

        (saved, removed)
    }

    /// L4: Auto Compact — LLM summarization of older messages.
    /// Returns (tokens_saved, evicted_messages, summary_option).
    ///
    /// Tries structured JSON output first; falls back to natural language on failure.
    /// On LLM failure, returns (0, vec![], None) so the pipeline falls through to L5.
    async fn apply_l4_compact(
        &self,
        messages: &mut Vec<Message>,
        llm: &dyn LlmClient,
        focus: Option<&str>,
    ) -> (usize, Vec<Message>, Option<String>) {
        let keep = self.config.l4_keep_recent;

        // Split into system / old / recent
        let system_msgs: Vec<Message> = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .cloned()
            .collect();
        let non_system: Vec<Message> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .cloned()
            .collect();

        if non_system.len() <= keep {
            return (0, vec![], None);
        }

        let split_at = non_system.len() - keep;
        let to_summarize = &non_system[..split_at];
        let to_keep = &non_system[split_at..];

        // Try structured output first if provider supports it
        let summary = if llm.capabilities().structured_output {
            let prompt =
                crate::compression::compressor::structured_summary_prompt(to_summarize, focus);
            match llm
                .chat(echo_core::llm::ChatRequest {
                    messages: vec![Message::user(prompt)],
                    temperature: Some(0.3),
                    max_tokens: Some(2048),
                    tools: None,
                    tool_choice: None,
                    response_format: Some(echo_core::llm::types::ResponseFormat::JsonObject),
                    thinking: None,
                    cancel_token: None,
                    user_id: None,
                    cache_hints: None,
                })
                .await
            {
                Ok(resp) => {
                    let text = resp.content().unwrap_or_default().to_string();
                    if let Some(parsed) = super::StructuredSummary::from_llm_response(&text) {
                        parsed.to_json()
                    } else {
                        // JSON parse failed — use raw text
                        text
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "L4 structured summary failed, falling back to natural language");
                    // Fall through
                    String::new()
                }
            }
        } else {
            String::new()
        };

        // Natural language fallback
        let summary = if summary.is_empty() {
            let prompt = crate::compression::compressor::default_summary_prompt_with_focus(
                to_summarize,
                focus,
            );
            match llm.chat_simple(vec![Message::user(prompt)]).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "L4 Auto-Compact: LLM summarization failed, falling through to L5");
                    return (0, vec![], None);
                }
            }
        } else {
            summary
        };

        // Calculate tokens saved
        let evicted_tokens: usize = to_summarize
            .iter()
            .map(|m| {
                self.tokenizer
                    .count_tokens(m.content.as_text_ref().unwrap_or(""))
            })
            .sum();
        let summary_tokens = self.tokenizer.count_tokens(&summary);
        let saved = evicted_tokens.saturating_sub(summary_tokens);

        let evicted = to_summarize.to_vec();

        // Rebuild messages: system + summary + recent
        let summary_clone = summary.clone();
        let mut new_messages = system_msgs;
        new_messages.push(Message::system(format!("[对话历史摘要]\n{}", summary)));
        new_messages.extend(to_keep.iter().cloned());
        *messages = new_messages;

        (saved, evicted, Some(summary_clone))
    }

    /// L5: Emergency — keep only system prompt + last 3 messages.
    /// Returns (tokens_saved, evicted_messages).
    fn apply_l5_reactive(&self, messages: &mut Vec<Message>) -> (usize, Vec<Message>) {
        let system_msgs: Vec<Message> = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .cloned()
            .collect();
        let recent: Vec<Message> = messages.iter().rev().take(3).rev().cloned().collect();

        let old_tokens: usize = messages
            .iter()
            .map(|m| {
                self.tokenizer
                    .count_tokens(m.content.as_text_ref().unwrap_or(""))
            })
            .sum();

        // Collect evicted: everything that is not system and not in recent (last 3 non-system)
        let non_system: Vec<Message> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .cloned()
            .collect();
        let evicted = if non_system.len() > 3 {
            non_system[..non_system.len() - 3].to_vec()
        } else {
            vec![]
        };

        let mut new_messages = system_msgs;
        new_messages.push(Message::system(
            "[Emergency compression: context was critically large. Only recent messages retained.]"
                .to_string(),
        ));
        new_messages.extend(recent);

        let new_tokens: usize = new_messages
            .iter()
            .map(|m| {
                self.tokenizer
                    .count_tokens(m.content.as_text_ref().unwrap_or(""))
            })
            .sum();

        *messages = new_messages;
        (old_tokens.saturating_sub(new_tokens), evicted)
    }
}

// ── ContextCompressor trait implementation ──────────────────────────────────────

impl super::ContextCompressor for AdaptiveCompressor {
    fn name(&self) -> &'static str {
        "Adaptive"
    }

    fn compress(
        &self,
        input: super::CompressionInput,
    ) -> BoxFuture<'_, echo_core::error::Result<super::CompressionOutput>> {
        Box::pin(async move {
            let start = Instant::now();
            let mut messages = input.messages;
            let target_tokens = input.token_limit;
            let focus = input.focus_instructions.clone();

            // Estimate current token count from the input messages
            let current_tokens: usize = messages
                .iter()
                .filter_map(|m| m.content.as_text())
                .map(|c| self.tokenizer.count_tokens(&c))
                .sum();

            let (result, evicted, l4_summary) = self
                .compress_async(
                    &mut messages,
                    current_tokens,
                    target_tokens,
                    focus.as_deref(),
                )
                .await;

            let tokens_after: usize = messages
                .iter()
                .filter_map(|m| m.content.as_text())
                .map(|c| self.tokenizer.count_tokens(&c))
                .sum();

            let mut checkpoint = CompressionCheckpoint::new(self.name())
                .with_counts(messages.len(), evicted.len())
                .with_tokens(result.tokens_before, tokens_after)
                .with_levels(result.levels_applied)
                .with_duration_ms(start.elapsed().as_millis() as u64)
                .with_focus(focus);
            if let Some(summary) = l4_summary {
                checkpoint = checkpoint.with_summary(summary);
            }

            Ok(super::CompressionOutput {
                messages,
                evicted,
                checkpoint: Some(checkpoint),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: MessageContent::Text(text.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_l1_snip() {
        let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig {
            l1_max_output_tokens: 10,
            l1_snip_threshold_tokens: 0,
            ..Default::default()
        });
        let mut messages = vec![
            make_msg(Role::User, "hello"),
            make_msg(Role::Tool, &"x".repeat(1000)),
        ];
        let result = compressor.compress_in_place(&mut messages, 500, 100);
        assert!(result.levels_applied.contains(&"L1:Snip".to_string()));
    }

    #[test]
    fn test_l3_collapse() {
        let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig {
            l3_collapse_threshold_tokens: 0,
            l3_keep_recent: 2,
            ..Default::default()
        });
        let mut messages = vec![
            make_msg(Role::System, "system prompt"),
            make_msg(Role::User, "msg1"),
            make_msg(Role::Assistant, "msg2"),
            make_msg(Role::User, "msg3"),
            make_msg(Role::Assistant, "msg4"),
            make_msg(Role::User, "msg5"),
        ];
        let result = compressor.compress_in_place(&mut messages, 1000, 100);
        assert!(result.levels_applied.contains(&"L3:Collapse".to_string()));
        // System message should be preserved
        assert!(messages.iter().any(|m| m.role == Role::System));
    }

    #[test]
    fn test_no_compression_needed() {
        let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig::default());
        let mut messages = vec![
            make_msg(Role::User, "hello"),
            make_msg(Role::Assistant, "hi"),
        ];
        let result = compressor.compress_in_place(&mut messages, 100, 200);
        assert!(result.levels_applied.is_empty());
    }

    #[test]
    fn test_safe_truncate_ascii() {
        assert_eq!(safe_truncate("hello world", 5), "hello");
        assert_eq!(safe_truncate("hello", 10), "hello");
        assert_eq!(safe_truncate("hello", 0), "");
    }

    #[test]
    fn test_safe_truncate_cjk_no_panic() {
        // "你好世界" = 4 CJK chars, each 3 bytes = 12 bytes total
        let s = "你好世界";
        // max_bytes=4 falls in the middle of the second char (byte 3..6)
        // Should return only "你" (3 bytes), not panic
        let result = safe_truncate(s, 4);
        assert_eq!(result, "你");

        // max_bytes=1 falls inside the first char
        let result = safe_truncate(s, 1);
        assert_eq!(result, "");

        // max_bytes=6 covers exactly "你好"
        let result = safe_truncate(s, 6);
        assert_eq!(result, "你好");
    }

    #[test]
    fn test_l1_snip_cjk_no_panic() {
        let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig {
            l1_max_output_tokens: 5,
            l1_snip_threshold_tokens: 0,
            ..Default::default()
        });
        // Create a tool message with CJK content that would cause char_limit
        // to fall in the middle of a multi-byte character
        let cjk_text = "你".repeat(100); // 100 CJK chars = 300 bytes
        let mut messages = vec![make_msg(Role::Tool, &cjk_text)];
        // Should not panic
        let result = compressor.compress_in_place(&mut messages, 500, 50);
        assert!(result.levels_applied.contains(&"L1:Snip".to_string()));
        // Verify the truncated content is valid UTF-8
        let text = messages[0].content.as_text_ref().unwrap();
        assert!(text.starts_with("你"));
    }

    #[test]
    fn test_l1_fold_consecutive_tools() {
        let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig {
            l1_snip_threshold_tokens: 0, // Always trigger L1
            l1_max_output_tokens: 99999, // Don't snip, only fold
            l1_fold_consecutive_tools: true,
            l1_fold_keep_latest: 2,
            ..Default::default()
        });

        let mut messages = vec![
            make_msg(Role::User, "do stuff"),
            make_msg(Role::Tool, "result1"),
            make_msg(Role::Tool, "result2"),
            make_msg(Role::Tool, "result3"),
            make_msg(Role::Tool, "result4"),
            make_msg(Role::Tool, "result5"),
            make_msg(Role::Assistant, "done"),
        ];
        // 5 tool messages with keep=2 should fold 3
        let result = compressor.compress_in_place(&mut messages, 500, 50);
        assert!(result.levels_applied.contains(&"L1:Fold".to_string()));
        // Original: 7 messages. Fold removes 3 tool msgs, inserts 1 fold msg => 7 - 3 + 1 = 5
        assert_eq!(messages.len(), 5);
        // The fold summary should be present
        assert!(
            messages
                .iter()
                .any(|m| m.content.as_text_ref().unwrap_or("").contains("L1 fold"))
        );
    }

    #[test]
    fn test_l1_fold_disabled() {
        let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig {
            l1_snip_threshold_tokens: 0,
            l1_max_output_tokens: 99999,
            l1_fold_consecutive_tools: false, // Disabled
            ..Default::default()
        });
        let mut messages = vec![
            make_msg(Role::Tool, "result1"),
            make_msg(Role::Tool, "result2"),
            make_msg(Role::Tool, "result3"),
        ];
        let result = compressor.compress_in_place(&mut messages, 500, 50);
        assert!(!result.levels_applied.contains(&"L1:Fold".to_string()));
        assert_eq!(messages.len(), 3); // No folding
    }

    #[tokio::test]
    async fn test_adaptive_as_context_compressor() {
        use crate::compression::ContextManager;

        let config = AdaptiveCompressionConfig {
            l1_snip_threshold_tokens: 0, // Always trigger L1
            l1_max_output_tokens: 5,
            l3_collapse_threshold_tokens: 0, // Always trigger L3 when over target
            l3_keep_recent: 2,
            ..Default::default()
        };
        let compressor = AdaptiveCompressor::new(config);

        // Use AdaptiveCompressor through ContextManager via ContextCompressor trait
        let mut ctx = ContextManager::builder(10) // very low token limit to trigger compression
            .compressor(compressor)
            .build();

        ctx.push(Message::system("system prompt".to_string()));
        for i in 0..10 {
            ctx.push(Message::user(format!("question {}", i)));
            ctx.push(Message::assistant(format!("answer {}", i)));
        }

        // prepare() should trigger auto-compression via the ContextCompressor trait
        let result = ctx.prepare(None).await.unwrap();
        // Compression should have been triggered since we have many messages
        // and the token limit is very low (50)
        assert!(
            result.compressed.is_some(),
            "compression should have been triggered"
        );
        let stats = result.compressed.unwrap();
        assert!(
            stats.before_count > stats.after_count,
            "messages should have been reduced"
        );
    }

    #[tokio::test]
    async fn test_adaptive_trait_evicted_messages() {
        use super::super::{CompressionInput, CompressionOutput, ContextCompressor};

        let config = AdaptiveCompressionConfig {
            l3_collapse_threshold_tokens: 0, // Always trigger L3
            l3_keep_recent: 2,
            ..Default::default()
        };
        let compressor = AdaptiveCompressor::new(config);

        let messages = vec![
            make_msg(Role::System, "system"),
            make_msg(Role::User, "msg1"),
            make_msg(Role::Assistant, "msg2"),
            make_msg(Role::User, "msg3"),
            make_msg(Role::Assistant, "msg4"),
            make_msg(Role::User, "msg5"),
        ];

        let input = CompressionInput {
            messages,
            token_limit: 0, // force compression
            current_query: None,
            focus_instructions: None,
        };

        let output: CompressionOutput = compressor.compress(input).await.unwrap();
        // L3 should have evicted older messages
        assert!(
            !output.evicted.is_empty(),
            "L3 should produce evicted messages"
        );
        assert!(
            output.messages.len() < 6,
            "output should have fewer messages"
        );
        // System message should be preserved
        assert!(output.messages.iter().any(|m| m.role == Role::System));
    }
}
