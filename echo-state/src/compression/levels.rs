//! Multi-level adaptive compression.
//!
//! Levels from cheapest to most expensive:
//! L1 Snip: Remove large tool outputs (>N tokens)
//! L2 Micro: Truncate tool outputs to keep first/last N lines
//! L3 Context Collapse: Summarize older message groups (non-LLM, rule-based)
//! L4 Auto Compact: Full LLM summarization (handled externally)
//! L5 Reactive: Emergency — keep only system prompt + last 3 messages

use echo_core::llm::types::{Message, MessageContent, Role};
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveCompressionConfig {
    /// Token threshold to trigger L1 (snip large outputs)
    pub l1_snip_threshold_tokens: usize,
    /// Max tokens per tool output before snipping
    pub l1_max_output_tokens: usize,
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
    #[allow(dead_code)]
    pub l4_keep_recent: usize,
}

impl Default for AdaptiveCompressionConfig {
    fn default() -> Self {
        Self {
            l1_snip_threshold_tokens: 80_000,
            l1_max_output_tokens: 4_000,
            l2_micro_threshold_tokens: 100_000,
            l2_keep_lines: 50,
            l3_collapse_threshold_tokens: 120_000,
            l3_keep_recent: 10,
            l4_compact_threshold_tokens: 150_000,
            l4_keep_recent: 6,
        }
    }
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
pub struct AdaptiveCompressor {
    config: AdaptiveCompressionConfig,
    tokenizer: HeuristicTokenizer,
}

impl AdaptiveCompressor {
    pub fn new(config: AdaptiveCompressionConfig) -> Self {
        Self {
            config,
            tokenizer: HeuristicTokenizer,
        }
    }

    /// Compress messages adaptively based on current token usage.
    pub fn compress(
        &self,
        messages: &mut Vec<Message>,
        current_tokens: usize,
        target_tokens: usize,
    ) -> AdaptiveCompressionResult {
        let tokens_before = current_tokens;
        let mut levels_applied = Vec::new();
        let mut tokens = current_tokens;

        // L1: Snip large tool outputs
        if tokens > self.config.l1_snip_threshold_tokens && tokens > target_tokens {
            let snipped = self.apply_l1_snip(messages);
            tokens = tokens.saturating_sub(snipped);
            if snipped > 0 {
                levels_applied.push("L1:Snip".to_string());
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
            let collapsed = self.apply_l3_collapse(messages);
            tokens = tokens.saturating_sub(collapsed);
            if collapsed > 0 {
                levels_applied.push("L3:Collapse".to_string());
            }
        }

        // L5: Reactive — emergency (L4 requires LLM, handled externally)
        if tokens > target_tokens * 2 && tokens > self.config.l4_compact_threshold_tokens {
            let saved = self.apply_l5_reactive(messages);
            tokens = tokens.saturating_sub(saved);
            if saved > 0 {
                levels_applied.push("L5:Reactive".to_string());
            }
        }

        AdaptiveCompressionResult {
            levels_applied,
            tokens_before,
            tokens_after: tokens,
        }
    }

    /// L1: Remove tool outputs that exceed max_output_tokens.
    fn apply_l1_snip(&self, messages: &mut Vec<Message>) -> usize {
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
                    &text[..text.len().min(char_limit)],
                    tokens,
                    max_tokens
                );
                saved += tokens - max_tokens;
                msg.content = MessageContent::Text(truncated);
            }
        }
        saved
    }

    /// L2: Truncate tool outputs to keep first/last N lines.
    fn apply_l2_micro(&self, messages: &mut Vec<Message>) -> usize {
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
    fn apply_l3_collapse(&self, messages: &mut Vec<Message>) -> usize {
        let keep = self.config.l3_keep_recent;
        if messages.len() <= keep + 1 {
            return 0;
        }

        // Keep system messages + last N messages
        let system_msgs: Vec<Message> = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .cloned()
            .collect();
        let recent: Vec<Message> = messages.iter().rev().take(keep).rev().cloned().collect();

        let removed: Vec<&Message> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .take(messages.len().saturating_sub(keep + system_msgs.len()))
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

        saved
    }

    /// L5: Emergency — keep only system prompt + last 3 messages.
    fn apply_l5_reactive(&self, messages: &mut Vec<Message>) -> usize {
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
        old_tokens.saturating_sub(new_tokens)
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
        let result = compressor.compress(&mut messages, 500, 100);
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
        let result = compressor.compress(&mut messages, 1000, 100);
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
        let result = compressor.compress(&mut messages, 100, 200);
        assert!(result.levels_applied.is_empty());
    }
}
