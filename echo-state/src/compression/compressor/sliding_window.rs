use crate::compression::{
    CompressionCheckpoint, CompressionInput, CompressionOutput, ContextCompressor,
};
use echo_core::error::Result;
use echo_core::llm::types::Role;
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use futures::future::BoxFuture;
use std::time::Instant;

/// 滑动窗口压缩：保留最近 `window_size` 条非 system 消息，裁掉更早的部分。
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
            let tokenizer = HeuristicTokenizer;
            let _total_messages = input.messages.len();
            let tokens_before: usize = input
                .messages
                .iter()
                .filter_map(|m| m.content.as_text())
                .map(|c| tokenizer.count_tokens(&c))
                .sum();

            let (system_msgs, conv_msgs): (Vec<_>, Vec<_>) = input
                .messages
                .into_iter()
                .partition(|m| m.role == Role::System);

            let system_count = system_msgs.len();

            if conv_msgs.len() <= self.window_size {
                let mut messages = system_msgs;
                messages.extend(conv_msgs);
                let tokens_after: usize = messages
                    .iter()
                    .filter_map(|m| m.content.as_text())
                    .map(|c| tokenizer.count_tokens(&c))
                    .sum();
                let checkpoint = CompressionCheckpoint::new(self.name())
                    .with_counts(messages.len(), 0)
                    .with_tokens(tokens_before, tokens_after)
                    .with_duration_ms(start.elapsed().as_millis() as u64)
                    .with_focus(input.focus_instructions.clone());
                return Ok(CompressionOutput {
                    messages,
                    evicted: vec![],
                    checkpoint: Some(checkpoint),
                });
            }

            let split_at = conv_msgs.len() - self.window_size;
            let evicted = conv_msgs[..split_at].to_vec();
            let kept = conv_msgs[split_at..].to_vec();

            let mut messages = system_msgs;
            messages.extend(kept);

            let tokens_after: usize = messages
                .iter()
                .filter_map(|m| m.content.as_text())
                .map(|c| tokenizer.count_tokens(&c))
                .sum();

            let checkpoint = CompressionCheckpoint::new(self.name())
                .with_covered_range(system_count, system_count + split_at - 1)
                .with_counts(messages.len(), evicted.len())
                .with_tokens(tokens_before, tokens_after)
                .with_duration_ms(start.elapsed().as_millis() as u64)
                .with_focus(input.focus_instructions.clone());

            Ok(CompressionOutput {
                messages,
                evicted,
                checkpoint: Some(checkpoint),
            })
        })
    }
}
