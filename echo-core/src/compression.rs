//! Context compression — trait and input/output types
//!
//! Implementations live in `echo_state::compression::compressor`.

use crate::error::Result;
use crate::llm::types::Message;
use futures::future::BoxFuture;

/// Compression pipeline input
pub struct CompressionInput {
    /// Messages to be compressed
    pub messages: Vec<Message>,
    /// Token limit, triggers compression when exceeded
    pub token_limit: usize,
    /// Current user query (reserved field for future extensions)
    pub current_query: Option<String>,
}

/// Compression pipeline output
pub struct CompressionOutput {
    /// Final list of messages to keep and send to the LLM
    pub messages: Vec<Message>,
    /// Messages evicted in this compression pass
    pub evicted: Vec<Message>,
}

/// Unified interface for all compression strategies (async, supports `dyn` trait object)
pub trait ContextCompressor: Send + Sync {
    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>>;
}

/// Allows `Box<dyn ContextCompressor>` to be passed directly to any function accepting
/// `impl ContextCompressor`, without introducing an extra wrapper enum.
impl ContextCompressor for Box<dyn ContextCompressor> {
    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        (**self).compress(input)
    }
}
