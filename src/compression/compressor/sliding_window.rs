use crate::compression::{CompressionInput, CompressionOutput, ContextCompressor};
use crate::error::Result;
use async_trait::async_trait;

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

#[async_trait]
impl ContextCompressor for SlidingWindowCompressor {
    async fn compress(&self, input: CompressionInput) -> Result<CompressionOutput> {
        let (system_msgs, conv_msgs): (Vec<_>, Vec<_>) =
            input.messages.into_iter().partition(|m| m.role == "system");

        if conv_msgs.len() <= self.window_size {
            let mut messages = system_msgs;
            messages.extend(conv_msgs);
            return Ok(CompressionOutput {
                messages,
                evicted: vec![],
            });
        }

        let split_at = conv_msgs.len() - self.window_size;
        let evicted = conv_msgs[..split_at].to_vec();
        let kept = conv_msgs[split_at..].to_vec();

        let mut messages = system_msgs;
        messages.extend(kept);

        Ok(CompressionOutput { messages, evicted })
    }
}
