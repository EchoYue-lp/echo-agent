use crate::compression::{
    CompressionCheckpoint, CompressionInput, CompressionOutput, ContextCompressor,
};
use echo_core::error::Result;
use echo_core::llm::types::Role;
use echo_core::tokenizer::Tokenizer;
use futures::future::BoxFuture;
use std::time::Instant;

/// 滑动窗口压缩：在 token 限额内保留最近至多 `window_size` 条非 system 消息。
///
/// - system 消息始终保留在列表最前面，不计入窗口计数
/// - 适用于高频、上下文独立的场景，或需要严格控制 token 成本的场景
pub struct SlidingWindowCompressor {
    window_size: usize,
}

impl SlidingWindowCompressor {
    pub fn new(window_size: usize) -> Self {
        Self { window_size }
    }
}

impl ContextCompressor for SlidingWindowCompressor {
    fn name(&self) -> &'static str {
        "SlidingWindow"
    }

    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        Box::pin(async move {
            let start = Instant::now();
            let tokenizer = input.tokenizer();
            let _total_messages = input.messages.len();
            let tokens_before = message_tokens(&input.messages, tokenizer.as_ref());

            let (system_msgs, mut conv_msgs): (Vec<_>, Vec<_>) = input
                .messages
                .into_iter()
                .partition(|m| m.role == Role::System);

            let system_count = system_msgs.len();

            let system_tokens = message_tokens(&system_msgs, tokenizer.as_ref());
            let conversation_limit = input.token_limit.saturating_sub(system_tokens);
            let mut kept_tokens = 0usize;
            let mut keep_from = conv_msgs.len();
            while keep_from > 0 && conv_msgs.len().saturating_sub(keep_from) < self.window_size {
                let mut group_start = keep_from.saturating_sub(1);
                if conv_msgs
                    .get(group_start)
                    .is_some_and(|message| message.role == Role::Tool)
                {
                    while group_start > 0
                        && conv_msgs
                            .get(group_start.saturating_sub(1))
                            .is_some_and(|message| message.role == Role::Tool)
                    {
                        group_start = group_start.saturating_sub(1);
                    }
                    if group_start > 0
                        && conv_msgs
                            .get(group_start.saturating_sub(1))
                            .is_some_and(|message| {
                                message.role == Role::Assistant && message.tool_calls.is_some()
                            })
                    {
                        group_start = group_start.saturating_sub(1);
                    }
                }
                let group = conv_msgs.get(group_start..keep_from).unwrap_or_default();
                let group_cost = message_tokens(group, tokenizer.as_ref());
                if kept_tokens.saturating_add(group_cost) > conversation_limit
                    || conv_msgs.len().saturating_sub(group_start) > self.window_size
                {
                    break;
                }
                kept_tokens = kept_tokens.saturating_add(group_cost);
                keep_from = group_start;
            }

            let split_at = keep_from;
            let kept = conv_msgs.split_off(split_at);
            let evicted = conv_msgs;

            let mut messages = system_msgs;
            messages.extend(kept);

            let tokens_after = message_tokens(&messages, tokenizer.as_ref());

            let mut checkpoint = CompressionCheckpoint::new(self.name())
                .with_counts(messages.len(), evicted.len())
                .with_tokens(tokens_before, tokens_after)
                .with_duration_ms(start.elapsed().as_millis() as u64)
                .with_focus(
                    input
                        .focus_instructions
                        .clone()
                        .or(input.current_query.clone()),
                );
            if split_at > 0 {
                checkpoint = checkpoint.with_covered_range(
                    system_count,
                    system_count.saturating_add(split_at).saturating_sub(1),
                );
            }

            Ok(CompressionOutput {
                messages,
                evicted,
                checkpoint: Some(checkpoint),
            })
        })
    }
}

fn message_tokens(messages: &[echo_core::llm::types::Message], tokenizer: &dyn Tokenizer) -> usize {
    messages.iter().fold(0usize, |total, message| {
        total.saturating_add(message.content.estimated_tokens(tokenizer))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::llm::types::{FunctionCall, Message, ToolCall};
    use echo_core::tokenizer::HeuristicTokenizer;

    #[tokio::test]
    async fn compression_is_token_bounded_and_keeps_tool_groups_atomic() -> Result<()> {
        let compressor = SlidingWindowCompressor::new(40);
        let mut call = Message::assistant(String::new());
        call.tool_calls = Some(vec![ToolCall {
            id: "call-1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "lookup".to_string(),
                arguments: "{}".to_string(),
            },
        }]);
        let result =
            Message::tool_result("call-1".to_string(), "lookup".to_string(), "r".repeat(400));
        let output = compressor
            .compress(CompressionInput {
                messages: vec![Message::user("old".repeat(100)), call, result],
                token_limit: 35,
                current_query: None,
                focus_instructions: None,
                cancel_token: None,
                tokenizer: None,
            })
            .await?;

        assert!(message_tokens(&output.messages, &HeuristicTokenizer) <= 35);
        assert!(
            output.messages.is_empty(),
            "oversized tool group must be evicted whole"
        );
        assert_eq!(output.evicted.len(), 3);

        let mut fitting_call = Message::assistant(String::new());
        fitting_call.tool_calls = Some(vec![ToolCall {
            id: "call-2".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "lookup".to_string(),
                arguments: "{}".to_string(),
            },
        }]);
        let fitting_result =
            Message::tool_result("call-2".to_string(), "lookup".to_string(), "ok".to_string());
        let fitting = compressor
            .compress(CompressionInput {
                messages: vec![
                    Message::user("old".repeat(100)),
                    fitting_call,
                    fitting_result,
                ],
                token_limit: 35,
                current_query: None,
                focus_instructions: None,
                cancel_token: None,
                tokenizer: None,
            })
            .await?;
        assert_eq!(fitting.messages.len(), 2);
        assert!(
            fitting
                .messages
                .first()
                .is_some_and(|message| message.tool_calls.is_some())
        );
        assert!(
            fitting
                .messages
                .get(1)
                .is_some_and(|message| message.role == Role::Tool)
        );
        Ok(())
    }
}
