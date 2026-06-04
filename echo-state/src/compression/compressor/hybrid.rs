use crate::compression::{CompressionInput, CompressionOutput, ContextCompressor};
use echo_core::error::Result;
use echo_core::llm::types::Message;
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use futures::future::BoxFuture;

/// Multi-stage pipeline compressor.
///
/// Chains multiple [`ContextCompressor`] implementations as an ordered pipeline.
/// Each stage's output feeds into the next stage's input.
///
/// **Short-circuit**: When enabled (default), the pipeline skips remaining stages
/// once the estimated token count drops to or below `token_limit`. This avoids
/// unnecessary LLM calls in later stages when earlier stages already suffice.
///
/// # Example
///
/// ```rust,no_run
/// use echo_core::llm::LlmClient;
/// use echo_state::compression::compressor::{
///     HybridCompressor, SlidingWindowCompressor, SummaryCompressor,
/// };
/// use std::sync::Arc;
///
/// # async fn example(llm: Arc<dyn LlmClient>) {
/// let compressor = HybridCompressor::builder()
///     .stage(SlidingWindowCompressor::new(20))
///     .stage(SummaryCompressor::new(llm, 8))
///     .build();
/// # }
/// ```
pub struct HybridCompressor {
    stages: Vec<Box<dyn ContextCompressor>>,
    /// When true, skip remaining stages if tokens are already at or below the limit.
    short_circuit: bool,
    tokenizer: HeuristicTokenizer,
}

impl ContextCompressor for HybridCompressor {
    fn name(&self) -> &'static str {
        "Hybrid"
    }

    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        Box::pin(async move {
            let token_limit = input.token_limit;
            let current_query = input.current_query.clone();
            let mut messages = input.messages;
            let mut all_evicted: Vec<Message> = Vec::new();

            for (i, stage) in self.stages.iter().enumerate() {
                // Short-circuit: if tokens are already at or below the limit, skip remaining stages
                if self.short_circuit && i > 0 {
                    let current_tokens: usize = messages
                        .iter()
                        .filter_map(|m| m.content.as_text())
                        .map(|c| self.tokenizer.count_tokens(&c))
                        .sum();
                    if current_tokens <= token_limit {
                        tracing::debug!(
                            stage = i,
                            total_stages = self.stages.len(),
                            current_tokens,
                            token_limit,
                            "HybridCompressor: short-circuiting, skipping remaining stages"
                        );
                        break;
                    }
                }

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
        })
    }
}

impl HybridCompressor {
    pub fn builder() -> HybridCompressorBuilder {
        HybridCompressorBuilder::default()
    }
}

/// Builder for [`HybridCompressor`]
#[derive(Default)]
pub struct HybridCompressorBuilder {
    stages: Vec<Box<dyn ContextCompressor>>,
    short_circuit: Option<bool>,
}

impl HybridCompressorBuilder {
    /// Append a compression stage (executed in order).
    pub fn stage(mut self, compressor: impl ContextCompressor + 'static) -> Self {
        self.stages.push(Box::new(compressor));
        self
    }

    /// Enable or disable short-circuit optimization.
    ///
    /// When enabled (default), the pipeline skips remaining stages once the
    /// estimated token count drops to or below `token_limit`. Set to `false`
    /// to always run all stages regardless.
    pub fn short_circuit(mut self, enabled: bool) -> Self {
        self.short_circuit = Some(enabled);
        self
    }

    pub fn build(self) -> HybridCompressor {
        HybridCompressor {
            stages: self.stages,
            short_circuit: self.short_circuit.unwrap_or(true),
            tokenizer: HeuristicTokenizer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::compressor::SlidingWindowCompressor;
    use echo_core::llm::types::{MessageContent, Role};

    fn make_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: MessageContent::Text(text.to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_hybrid_short_circuit_skips_second_stage() {
        // Two sliding window stages: first keeps 10, second keeps 2
        // With short_circuit=true and token_limit=100 (high enough),
        // the first stage should run and the second should be skipped
        // because tokens after first stage should be below the high limit.
        let compressor = HybridCompressor::builder()
            .stage(SlidingWindowCompressor::new(10))
            .stage(SlidingWindowCompressor::new(2))
            .short_circuit(true)
            .build();

        let mut messages = vec![make_msg(Role::System, "system")];
        for i in 0..15 {
            messages.push(make_msg(Role::User, &format!("msg {}", i)));
        }

        let input = CompressionInput {
            messages,
            token_limit: 1000, // Very high — first stage result will be under this
            current_query: None,
        };

        let output = compressor.compress(input).await.unwrap();
        // With short_circuit, second stage (keep 2) should be skipped.
        // First stage keeps 10 messages, so output should have 10 + 1 system = 11.
        // Without short_circuit, second stage would reduce to 2 + 1 = 3.
        assert!(
            output.messages.len() > 3,
            "Short-circuit should have prevented second stage from running. Got {} messages.",
            output.messages.len()
        );
    }

    #[tokio::test]
    async fn test_hybrid_no_short_circuit_runs_all_stages() {
        let compressor = HybridCompressor::builder()
            .stage(SlidingWindowCompressor::new(10))
            .stage(SlidingWindowCompressor::new(2))
            .short_circuit(false)
            .build();

        let mut messages = vec![make_msg(Role::System, "system")];
        for i in 0..15 {
            messages.push(make_msg(Role::User, &format!("msg {}", i)));
        }

        let input = CompressionInput {
            messages,
            token_limit: 1000,
            current_query: None,
        };

        let output = compressor.compress(input).await.unwrap();
        // Both stages should run: first keeps 10, second keeps 2
        // Result: system + 2 = 3 messages
        assert_eq!(output.messages.len(), 3, "All stages should have run");
    }
}
