//! Visibility Horizon — sliding-window tool trace compaction.
//!
//! # Three-layer visibility model
//!
//! | Layer | Lifetime | Content | Treatment |
//! |-------|----------|---------|-----------|
//! | **Global Objective** | permanent | user's original task goal | injected as system message, never compressed |
//! | **Active Plan Window** | last N turns | active conversation context | kept intact |
//! | **Transient Tool Trace** | beyond window | completed tool call + result pairs | compacted to symbolic summary |
//!
//! # How it works
//!
//! ```text
//! Messages: [sys] [u₁] [asst+tc] [tool] [tool] [u₂] [asst+tc] [tool] [u₃] ...
//!                   ↑ Turn 1  ← ToolGroup 1 →        ← ToolGroup 2 →  ↑ Turn 3
//!
//! With active_window_turns = 2:
//!   ToolGroup 1 has 2 user turns after it (u₂, u₃) → ≥ threshold → COMPACT
//!   ToolGroup 2 has 1 user turn after it (u₃)      → < threshold → KEEP
//! ```
//!
//! # Integration
//!
//! Use as a standalone [`ContextCompressor`] or integrate into
//! [`ContextManager`](crate::compression::ContextManager) as a pre-compression pass.

use echo_core::compression::{CompressionInput, CompressionOutput, ContextCompressor};
use echo_core::error::Result;
use echo_core::llm::types::{Message, MessageContent, Role};
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

// ── Configuration ────────────────────────────────────────────────────

/// Configuration for the visibility horizon compressor.
///
/// Controls when and how tool traces are compacted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisibilityHorizonConfig {
    /// Number of recent user turns to keep intact.
    ///
    /// Tool groups that have **more than** this many user messages after them
    /// are considered beyond the active window and will be compacted.
    #[serde(default = "default_active_window")]
    pub active_window_turns: usize,

    /// Maximum token budget for each compacted summary.
    #[serde(default = "default_compact_max_tokens")]
    pub compact_max_tokens: usize,

    /// Whether to inject a Global Objective system message.
    #[serde(default)]
    pub enable_global_objective: bool,

    /// The Global Objective text (user's original task goal).
    ///
    /// When `enable_global_objective` is true, this is injected as the
    /// first system message and survives all compression.
    #[serde(default)]
    pub global_objective: Option<String>,
}

fn default_active_window() -> usize {
    5
}
fn default_compact_max_tokens() -> usize {
    50
}

impl Default for VisibilityHorizonConfig {
    fn default() -> Self {
        Self {
            active_window_turns: 5,
            compact_max_tokens: 50,
            enable_global_objective: false,
            global_objective: None,
        }
    }
}

// ── ToolGroup (internal) ─────────────────────────────────────────────

/// A group of assistant tool_calls + their corresponding tool results.
struct ToolGroup {
    /// Index of the assistant message with `tool_calls`.
    assistant_idx: usize,
    /// Index range `[start, end)` of the tool result messages.
    result_start: usize,
    result_end: usize,
    /// Tool names extracted from the assistant's `tool_calls`.
    tool_names: Vec<String>,
    /// Number of successful tool results in this group.
    success_count: usize,
    /// Total number of tool results in this group.
    total_count: usize,
    /// Estimated token count of the tool result messages.
    result_tokens: usize,
    /// Error messages (if any).
    errors: Vec<String>,
    /// How many `Role::User` messages appear after this group.
    user_turns_after: usize,
}

// ── VisibilityHorizonCompressor ──────────────────────────────────────

/// Compresses tool traces beyond the active plan window into symbolic summaries.
///
/// Implements [`ContextCompressor`] for composability with other strategies
/// (e.g. via [`HybridCompressor`](crate::compression::compressor::HybridCompressor)).
pub struct VisibilityHorizonCompressor {
    config: VisibilityHorizonConfig,
    tokenizer: Box<dyn Tokenizer>,
}

impl VisibilityHorizonCompressor {
    pub fn new(config: VisibilityHorizonConfig) -> Self {
        Self {
            config,
            tokenizer: Box::new(HeuristicTokenizer),
        }
    }

    /// Core compaction: identify tool groups beyond the window, replace with summaries.
    ///
    /// Returns the evicted (original) messages for downstream memory promotion.
    fn compact_horizon(&self, messages: &mut Vec<Message>) -> Vec<Message> {
        let groups = self.identify_tool_groups(messages);

        // Filter groups beyond the active window
        let to_compact: Vec<&ToolGroup> = groups
            .iter()
            .filter(|g| g.user_turns_after > self.config.active_window_turns)
            .collect();

        if to_compact.is_empty() {
            return vec![];
        }

        let mut evicted = Vec::new();

        // Process from back to front to preserve indices
        for group in to_compact.into_iter().rev() {
            // 1. Build summary message
            let summary_text = self.build_compact_summary(group);
            let summary_msg = Message::user(summary_text);

            // 2. Strip tool_calls from the assistant message (keep reasoning text)
            if let Some(asst) = messages.get_mut(group.assistant_idx) {
                // Preserve the assistant's text content as a user-visible note
                let original_text = asst
                    .content
                    .as_text()
                    .unwrap_or_default()
                    .to_string();
                asst.tool_calls = None;
                if original_text.trim().is_empty() {
                    // No text — convert to a brief note
                    asst.content = MessageContent::Text(format!(
                        "[Used tools: {}]",
                        group.tool_names.join(", ")
                    ));
                }
            }

            // 3. Collect evicted tool result messages
            for i in group.result_start..group.result_end {
                if i < messages.len() {
                    evicted.push(messages[i].clone());
                }
            }

            // 4. Replace tool result range with a single summary message
            if group.result_start < messages.len() && group.result_end <= messages.len() {
                let drain_range = group.result_start..group.result_end;
                let drain_len = drain_range.end - drain_range.start;
                // Drain the old tool results and insert the summary
                let _: Vec<_> = messages.drain(drain_range).collect();
                messages.insert(group.result_start, summary_msg);

                // Note: we don't need to adjust indices of earlier groups
                // because we process back-to-front.
                let _ = drain_len; // suppress unused warning
            }
        }

        // 5. Inject Global Objective if configured
        if self.config.enable_global_objective {
            if let Some(ref objective) = self.config.global_objective {
                self.inject_global_objective(messages, objective);
            }
        }

        evicted
    }

    /// Identify all tool-call groups in the message list.
    fn identify_tool_groups(&self, messages: &[Message]) -> Vec<ToolGroup> {
        let mut groups = Vec::new();
        let mut i = 0;

        while i < messages.len() {
            // Look for assistant messages with tool_calls
            if messages[i].role == Role::Assistant {
                if let Some(ref tool_calls) = messages[i].tool_calls {
                    if tool_calls.is_empty() {
                        i += 1;
                        continue;
                    }

                    let assistant_idx = i;
                    let tool_names: Vec<String> = tool_calls
                        .iter()
                        .map(|tc| tc.function.name.clone())
                        .collect();

                    // Scan forward for tool result messages
                    let result_start = i + 1;
                    let mut result_end = result_start;
                    let mut success_count = 0;
                    let mut total_count = 0;
                    let mut result_tokens = 0;
                    let mut errors = Vec::new();

                    while result_end < messages.len()
                        && messages[result_end].role == Role::Tool
                    {
                        let content = messages[result_end]
                            .content
                            .as_text()
                            .unwrap_or_default();
                        result_tokens += self.tokenizer.count_tokens(&content);

                        // Heuristic: if content starts with "[Error" or "[error", count as failure
                        if content.trim_start().starts_with("[Error")
                            || content.trim_start().starts_with("[error")
                        {
                            errors.push(content.chars().take(100).collect());
                        } else {
                            success_count += 1;
                        }
                        total_count += 1;
                        result_end += 1;
                    }

                    // Count user turns after this group
                    let user_turns_after = messages[result_end..]
                        .iter()
                        .filter(|m| m.role == Role::User)
                        .count();

                    groups.push(ToolGroup {
                        assistant_idx,
                        result_start,
                        result_end,
                        tool_names,
                        success_count,
                        total_count,
                        result_tokens,
                        errors,
                        user_turns_after,
                    });

                    i = result_end;
                    continue;
                }
            }
            i += 1;
        }

        groups
    }

    /// Build a compact summary string for a tool group.
    fn build_compact_summary(&self, group: &ToolGroup) -> String {
        let tool_names = group.tool_names.join(", ");
        let status = if group.total_count == 0 {
            "no results".to_string()
        } else if group.success_count == group.total_count {
            format!("{}/{} success", group.success_count, group.total_count)
        } else {
            let err_preview = if group.errors.is_empty() {
                String::new()
            } else {
                format!(
                    ", error: \"{}\"",
                    group.errors[0].chars().take(60).collect::<String>()
                )
            };
            format!(
                "{}/{} success{}",
                group.success_count, group.total_count, err_preview
            )
        };

        let summary = format!(
            "[Horizon compact: {} | {} | {}→~{} tokens]",
            tool_names,
            status,
            group.result_tokens,
            self.config.compact_max_tokens
        );

        // Ensure summary stays within token budget
        let max_chars = self.config.compact_max_tokens * 4; // rough chars→tokens
        if summary.len() > max_chars {
            format!(
                "[Horizon compact: {} | {} | {} tokens compacted]",
                if group.tool_names.len() > 3 {
                    format!("{} tools", group.tool_names.len())
                } else {
                    tool_names
                },
                status,
                group.result_tokens
            )
        } else {
            summary
        }
    }

    /// Inject (or update) the Global Objective as the first system message.
    fn inject_global_objective(&self, messages: &mut Vec<Message>, objective: &str) {
        let objective_marker = "[Global Objective]";
        let objective_text = format!("{} {}", objective_marker, objective);

        // Check if already injected
        if let Some(first) = messages.first() {
            if first.role == Role::System
                && first
                    .content
                    .as_text()
                    .is_some_and(|t| t.contains(objective_marker))
            {
                return; // already present
            }
        }

        // Insert at the beginning
        messages.insert(0, Message::system(objective_text));
    }
}

impl ContextCompressor for VisibilityHorizonCompressor {
    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        Box::pin(async move {
            let mut messages = input.messages;
            let evicted = self.compact_horizon(&mut messages);
            Ok(CompressionOutput { messages, evicted })
        })
    }

    fn name(&self) -> &'static str {
        "VisibilityHorizon"
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::llm::types::ToolCall;

    /// Helper: create an assistant message with tool_calls.
    fn assistant_with_tools(calls: &[(&str, &str)]) -> Message {
        let tool_calls: Vec<ToolCall> = calls
            .iter()
            .map(|(id, name)| ToolCall {
                id: id.to_string(),
                call_type: "function".to_string(),
                function: echo_core::llm::types::FunctionCall {
                    name: name.to_string(),
                    arguments: "{}".to_string(),
                },
            })
            .collect();
        Message::assistant_with_tools(tool_calls)
    }

    /// Helper: create a tool result message.
    fn tool_result(id: &str, name: &str, content: &str) -> Message {
        Message::tool_result(id.to_string(), name.to_string(), content.to_string())
    }

    /// Build a multi-turn conversation with tool calls.
    /// Each turn produces: user → assistant(2 tool_calls) → 2 tool_results → assistant_answer
    fn build_conversation(turns: usize) -> Vec<Message> {
        let mut messages = vec![Message::system("You are a helpful assistant.".to_string())];

        for i in 1..=turns {
            // User message
            messages.push(Message::user(format!("Question {}", i)));

            // Assistant with 2 parallel tool calls
            let call_id_a = format!("call_{}a", i);
            let call_id_b = format!("call_{}b", i);
            let tool_a = format!("read_file_{}", i);
            let tool_b = format!("grep_{}", i);
            messages.push(assistant_with_tools(&[
                (&call_id_a, &tool_a),
                (&call_id_b, &tool_b),
            ]));

            // Two tool results (simulating parallel execution)
            messages.push(tool_result(&call_id_a, &tool_a, &"file content ".repeat(100 * i)));
            messages.push(tool_result(&call_id_b, &tool_b, &"grep output ".repeat(80 * i)));

            // Assistant final answer
            messages.push(Message::assistant(format!("Answer to question {}", i)));
        }

        messages
    }

    #[test]
    fn test_no_compaction_within_window() {
        let config = VisibilityHorizonConfig {
            active_window_turns: 5,
            ..Default::default()
        };
        let compressor = VisibilityHorizonCompressor::new(config);

        // 3-turn conversation — all within window of 5
        let mut messages = build_conversation(3);
        let original_len = messages.len();

        let evicted = compressor.compact_horizon(&mut messages);

        assert!(evicted.is_empty(), "No messages should be evicted");
        assert_eq!(
            messages.len(),
            original_len,
            "Message count should be unchanged"
        );
    }

    #[test]
    fn test_compaction_beyond_window() {
        let config = VisibilityHorizonConfig {
            active_window_turns: 2,
            compact_max_tokens: 50,
            ..Default::default()
        };
        let compressor = VisibilityHorizonCompressor::new(config);

        // 6-turn conversation with window=2 → turns 1-3 should be compacted
        let mut messages = build_conversation(6);
        let original_len = messages.len();

        let evicted = compressor.compact_horizon(&mut messages);

        assert!(
            !evicted.is_empty(),
            "Some tool results should be evicted"
        );
        assert!(
            messages.len() < original_len,
            "Message count should decrease: was {}, now {}",
            original_len,
            messages.len()
        );

        // Verify compacted messages contain horizon summary
        let has_summary = messages
            .iter()
            .any(|m| m.content.as_text().is_some_and(|t| t.contains("[Horizon compact")));
        assert!(has_summary, "Should contain a horizon compact summary");

        // Verify recent turns (4-6) are NOT compacted — tool results still present
        let has_recent_tool = messages
            .iter()
            .any(|m| m.role == Role::Tool);
        assert!(has_recent_tool, "Recent tool results should be preserved");
    }

    #[test]
    fn test_empty_messages() {
        let config = VisibilityHorizonConfig::default();
        let compressor = VisibilityHorizonCompressor::new(config);

        let mut messages: Vec<Message> = vec![];
        let evicted = compressor.compact_horizon(&mut messages);

        assert!(evicted.is_empty());
        assert!(messages.is_empty());
    }

    #[test]
    fn test_no_tool_messages() {
        let config = VisibilityHorizonConfig::default();
        let compressor = VisibilityHorizonCompressor::new(config);

        let mut messages = vec![
            Message::system("sys".to_string()),
            Message::user("hello".to_string()),
            Message::assistant("hi".to_string()),
            Message::user("how are you?".to_string()),
            Message::assistant("fine".to_string()),
        ];
        let original_len = messages.len();

        let evicted = compressor.compact_horizon(&mut messages);

        assert!(evicted.is_empty());
        assert_eq!(messages.len(), original_len);
    }

    #[test]
    fn test_parallel_tool_calls() {
        let config = VisibilityHorizonConfig {
            active_window_turns: 1,
            ..Default::default()
        };
        let compressor = VisibilityHorizonCompressor::new(config);

        let mut messages = vec![
            Message::system("sys".to_string()),
            // Turn 1: user asks, assistant calls 2 tools in parallel
            Message::user("do stuff".to_string()),
            assistant_with_tools(&[("call_a", "read_file"), ("call_b", "grep")]),
            tool_result("call_a", "read_file", &"file content".repeat(100)),
            tool_result("call_b", "grep", &"grep output".repeat(100)),
            Message::assistant("done".to_string()),
            // Turn 2: user follows up
            Message::user("thanks".to_string()),
            Message::assistant("welcome".to_string()),
            // Turn 3: another turn to push turn 1 beyond window
            Message::user("bye".to_string()),
            Message::assistant("bye".to_string()),
        ];

        let evicted = compressor.compact_horizon(&mut messages);

        // Turn 1's tool group has 2 user turns after it → > 1 → should compact
        assert!(!evicted.is_empty(), "Parallel tool results should be evicted");

        // Verify the summary mentions both tools
        let has_summary = messages.iter().any(|m| {
            m.content
                .as_text()
                .is_some_and(|t| t.contains("read_file") && t.contains("grep"))
        });
        assert!(has_summary, "Summary should mention both parallel tools");
    }

    #[test]
    fn test_mixed_success_failure() {
        let config = VisibilityHorizonConfig {
            active_window_turns: 0,
            ..Default::default()
        };
        let compressor = VisibilityHorizonCompressor::new(config);

        let mut messages = vec![
            Message::system("sys".to_string()),
            Message::user("run commands".to_string()),
            assistant_with_tools(&[("call_ok", "shell"), ("call_err", "shell")]),
            tool_result("call_ok", "shell", "command succeeded"),
            tool_result("call_err", "shell", "[Error] Permission denied"),
            Message::assistant("one failed".to_string()),
            // Need at least 1 user turn after for window=0
            Message::user("ok".to_string()),
        ];

        let evicted = compressor.compact_horizon(&mut messages);
        assert!(!evicted.is_empty());

        // Summary should indicate partial success
        let summary = messages
            .iter()
            .find(|m| m.content.as_text().is_some_and(|t| t.contains("[Horizon compact")))
            .and_then(|m| m.content.as_text())
            .unwrap_or_default();

        assert!(
            summary.contains("1/2 success"),
            "Summary should show 1/2 success, got: {}",
            summary
        );
    }

    #[test]
    fn test_preserves_non_tool_messages() {
        let config = VisibilityHorizonConfig {
            active_window_turns: 0,
            ..Default::default()
        };
        let compressor = VisibilityHorizonCompressor::new(config);

        let mut messages = vec![
            Message::system("sys".to_string()),
            Message::user("q1".to_string()),
            assistant_with_tools(&[("c1", "tool_a")]),
            tool_result("c1", "tool_a", "result_a"),
            Message::assistant("important reasoning text".to_string()),
            Message::user("q2".to_string()),
            Message::assistant("answer 2".to_string()),
        ];

        let _evicted = compressor.compact_horizon(&mut messages);

        // The assistant's "important reasoning text" should be preserved
        let has_reasoning = messages
            .iter()
            .any(|m| m.content.as_text().is_some_and(|t| t.contains("important reasoning")));
        assert!(has_reasoning, "Non-tool assistant messages should be preserved");

        // User messages should be preserved
        let user_count = messages.iter().filter(|m| m.role == Role::User).count();
        assert!(user_count >= 2, "User messages should be preserved");
    }

    #[test]
    fn test_global_objective_injection() {
        let config = VisibilityHorizonConfig {
            active_window_turns: 0, // compact all tool groups
            enable_global_objective: true,
            global_objective: Some("Build a sorting algorithm".to_string()),
            ..Default::default()
        };
        let compressor = VisibilityHorizonCompressor::new(config);

        let mut messages = vec![
            Message::system("You are a helper.".to_string()),
            Message::user("start".to_string()),
            assistant_with_tools(&[("c1", "tool_a")]),
            tool_result("c1", "tool_a", "result"),
            Message::assistant("done".to_string()),
            Message::user("next".to_string()),
        ];

        let evicted = compressor.compact_horizon(&mut messages);

        // Compaction should have happened (1 user turn after > 0 threshold)
        assert!(!evicted.is_empty(), "Tool results should be evicted");

        // Global objective should be injected as first message
        assert!(messages[0]
            .content
            .as_text()
            .is_some_and(|t| t.contains("[Global Objective]")));
        assert!(messages[0]
            .content
            .as_text()
            .is_some_and(|t| t.contains("Build a sorting algorithm")));
    }

    #[tokio::test]
    async fn test_context_compressor_trait() {
        let config = VisibilityHorizonConfig {
            active_window_turns: 1,
            ..Default::default()
        };
        let compressor = VisibilityHorizonCompressor::new(config);

        let messages = build_conversation(4);
        let original_count = messages.len();

        let input = CompressionInput {
            messages,
            token_limit: 100_000,
            current_query: None,
        };

        let output = compressor.compress(input).await.unwrap();

        assert!(
            output.messages.len() < original_count,
            "Should have fewer messages after compression"
        );
        assert!(!output.evicted.is_empty(), "Should have evicted messages");
        assert_eq!(compressor.name(), "VisibilityHorizon");
    }

    #[test]
    fn test_token_stability_over_25_turns() {
        // RFC 3.3 acceptance criterion: context token count remains stable
        // as conversation grows beyond 20 turns.
        let config = VisibilityHorizonConfig {
            active_window_turns: 5,
            compact_max_tokens: 50,
            ..Default::default()
        };
        let compressor = VisibilityHorizonCompressor::new(config);
        let tokenizer = HeuristicTokenizer;

        // Build conversations with CONSTANT-SIZE tool outputs to isolate
        // the effect of turn count on token growth.
        fn build_constant_conversation(turns: usize) -> Vec<Message> {
            let mut messages =
                vec![Message::system("You are a helpful assistant.".to_string())];
            for i in 1..=turns {
                messages.push(Message::user(format!("Question {}", i)));
                let ca = format!("call_{}a", i);
                let cb = format!("call_{}b", i);
                messages.push(assistant_with_tools(&[
                    (&ca, &format!("read_{}", i)),
                    (&cb, &format!("grep_{}", i)),
                ]));
                // Constant-size output (200 chars each)
                messages.push(tool_result(&ca, &format!("read_{}", i), &"x".repeat(200)));
                messages.push(tool_result(&cb, &format!("grep_{}", i), &"y".repeat(200)));
                messages.push(Message::assistant(format!("Answer {}", i)));
            }
            messages
        }

        // Measure tokens after compaction at various turn counts
        let mut token_counts: Vec<(usize, usize)> = Vec::new();

        for turns in [5, 10, 15, 20, 25] {
            let mut messages = build_constant_conversation(turns);
            let _evicted = compressor.compact_horizon(&mut messages);

            let total_tokens: usize = messages
                .iter()
                .filter_map(|m| m.content.as_text())
                .map(|c| tokenizer.count_tokens(&c))
                .sum();

            token_counts.push((turns, total_tokens));
        }

        // Key invariant: with active_window=5, only the last ~5 turns are kept
        // in full detail. Older turns are compacted to ~50 token summaries.
        // So token growth from 10→25 turns should be sub-linear.
        let tokens_at_10 = token_counts[1].1;
        let tokens_at_25 = token_counts[4].1;

        assert!(
            tokens_at_25 < tokens_at_10 * 2,
            "Token growth should be sub-linear with compaction: 10 turns={} tokens, 25 turns={} tokens",
            tokens_at_10,
            tokens_at_25
        );
    }

    #[test]
    fn test_30_round_integration() {
        // RFC 3.3 acceptance criterion: 30 rounds of tool calls with horizon compaction.
        let config = VisibilityHorizonConfig {
            active_window_turns: 5,
            compact_max_tokens: 50,
            ..Default::default()
        };
        let compressor = VisibilityHorizonCompressor::new(config);

        let mut messages = build_conversation(30);
        let original_count = messages.len();

        // Each turn produces 5 messages: user, assistant(2tc), tool, tool, assistant_answer
        // So 30 turns = 150 messages + 1 system = 151
        assert_eq!(original_count, 151, "Expected 151 messages for 30 turns");

        let evicted = compressor.compact_horizon(&mut messages);

        // With active_window=5 and 30 turns:
        //   Turns 1-24 have >5 user turns after → compacted
        //   Turns 25-30 have ≤5 user turns after → preserved (6 turns × 2 tools = 12 results)
        assert!(
            evicted.len() >= 40,
            "Should evict at least 40 tool result messages, got {}",
            evicted.len()
        );
        assert!(
            messages.len() < original_count,
            "Message count should decrease: was {}, now {}",
            original_count,
            messages.len()
        );

        // Verify recent turns preserve real tool results
        let recent_tool_results: usize = messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .count();
        // 6 preserved turns × 2 tool results = 12
        assert!(
            recent_tool_results >= 10,
            "At least 10 recent tool results should be preserved, got {}",
            recent_tool_results
        );

        // Verify no orphaned assistant tool_calls without matching results
        let mut orphaned = 0;
        for (i, msg) in messages.iter().enumerate() {
            if msg.role == Role::Assistant && msg.tool_calls.is_some() {
                let tc_ids: Vec<&str> = msg
                    .tool_calls
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|tc| tc.id.as_str())
                    .collect();
                for id in &tc_ids {
                    let has_result = messages[i + 1..]
                        .iter()
                        .take(10)
                        .any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some(id));
                    if !has_result {
                        orphaned += 1;
                    }
                }
            }
        }
        assert_eq!(
            orphaned, 0,
            "No orphaned tool_calls should exist after compaction"
        );

        // Verify compacted summaries are present
        let summary_count = messages
            .iter()
            .filter(|m| m.content.as_text().is_some_and(|t| t.contains("[Horizon compact")))
            .count();
        assert!(
            summary_count >= 20,
            "Should have at least 20 compact summaries, got {}",
            summary_count
        );
    }

    #[test]
    fn test_50_turn_token_stability() {
        // RFC 5.2.1: Long conversations (>50 turns) should maintain stable token count
        // with horizon compaction active.
        let config = VisibilityHorizonConfig {
            active_window_turns: 5,
            compact_max_tokens: 50,
            ..Default::default()
        };
        let compressor = VisibilityHorizonCompressor::new(config);
        let tokenizer = HeuristicTokenizer;

        // Constant-size outputs for predictable measurement
        fn build_constant_conversation(turns: usize) -> Vec<Message> {
            let mut messages =
                vec![Message::system("You are a helpful assistant.".to_string())];
            for i in 1..=turns {
                messages.push(Message::user(format!("Question {} about various topics", i)));
                let ca = format!("call_{}a", i);
                let cb = format!("call_{}b", i);
                messages.push(assistant_with_tools(&[
                    (&ca, &format!("read_{}", i)),
                    (&cb, &format!("grep_{}", i)),
                ]));
                messages.push(tool_result(&ca, &format!("read_{}", i), &"x".repeat(200)));
                messages.push(tool_result(&cb, &format!("grep_{}", i), &"y".repeat(200)));
                messages.push(Message::assistant(format!("Answer to question {}", i)));
            }
            messages
        }

        // Measure tokens at 50 turns vs 10 turns
        let mut msgs_10 = build_constant_conversation(10);
        let _ = compressor.compact_horizon(&mut msgs_10);
        let tokens_10: usize = msgs_10
            .iter()
            .filter_map(|m| m.content.as_text())
            .map(|c| tokenizer.count_tokens(&c))
            .sum();

        let mut msgs_50 = build_constant_conversation(50);
        let _ = compressor.compact_horizon(&mut msgs_50);
        let tokens_50: usize = msgs_50
            .iter()
            .filter_map(|m| m.content.as_text())
            .map(|c| tokenizer.count_tokens(&c))
            .sum();

        // With active_window=5, only the last ~5 turns are fully preserved.
        // Without compaction, 50 turns × ~500 tokens/turn = ~25,000 tokens.
        // With compaction, compacted turns become ~50 token summaries.
        // The compacted size should be much smaller than uncompressed.
        let uncompressed_50 = 50 * 500; // rough estimate: 50 turns × ~500 tokens each
        assert!(
            tokens_50 < uncompressed_50 / 3,
            "Compacted 50-turn conversation ({} tokens) should be <33% of uncompressed estimate ({} tokens)",
            tokens_50,
            uncompressed_50
        );

        // Verify compaction happened for old turns
        let compact_count = msgs_50
            .iter()
            .filter(|m| m.content.as_text().is_some_and(|t| t.contains("[Horizon compact")))
            .count();
        assert!(
            compact_count >= 40,
            "At least 40 groups should be compacted in 50 turns, got {}",
            compact_count
        );
    }
}
