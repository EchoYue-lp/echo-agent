use crate::compression::{CompressionInput, CompressionOutput, ContextCompressor};
use crate::error::Result;
use crate::llm::types::Message;
use async_trait::async_trait;

/// 混合压缩：将多个 `ContextCompressor` 串联为有序管道
///
/// 执行顺序：按 `stage()` 添加顺序依次执行，每个 stage 的输出作为下一个 stage 的输入。
///
/// # 示例
///
/// ```rust
/// let compressor = HybridCompressor::builder()
///     .stage(SlidingWindowCompressor::new(20))
///     .stage(SummaryCompressor::new(llm, DefaultSummaryPrompt, 8))
///     .build();
/// ```
pub struct HybridCompressor {
    stages: Vec<Box<dyn ContextCompressor>>,
}

#[async_trait]
impl ContextCompressor for HybridCompressor {
    async fn compress(&self, input: CompressionInput) -> Result<CompressionOutput> {
        let token_limit = input.token_limit;
        let current_query = input.current_query.clone();
        let mut messages = input.messages;
        let mut all_evicted: Vec<Message> = Vec::new();

        for stage in &self.stages {
            let output = stage
                .compress(CompressionInput {
                    messages,
                    token_limit,
                    current_query: current_query.clone(),
                })
                .await?;
            all_evicted.extend(output.evicted);
            messages = output.messages;
        }

        Ok(CompressionOutput {
            messages,
            evicted: all_evicted,
        })
    }
}

impl HybridCompressor {
    pub fn builder() -> HybridCompressorBuilder {
        HybridCompressorBuilder::default()
    }
}

/// `HybridCompressor` 的构建器
#[derive(Default)]
pub struct HybridCompressorBuilder {
    stages: Vec<Box<dyn ContextCompressor>>,
}

impl HybridCompressorBuilder {
    /// 追加一个压缩阶段（按调用顺序依次执行）
    pub fn stage(mut self, compressor: impl ContextCompressor + 'static) -> Self {
        self.stages.push(Box::new(compressor));
        self
    }

    pub fn build(self) -> HybridCompressor {
        HybridCompressor {
            stages: self.stages,
        }
    }
}
