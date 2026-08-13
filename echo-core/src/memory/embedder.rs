//! Text embedding interface
//!
//! Maps text to dense float vectors for semantic search.
//! Concrete implementation (`HttpEmbedder`) lives in `echo_state`.

use crate::error::Result;
use futures::future::BoxFuture;

/// Text embedding interface: maps text to dense float vectors
pub trait Embedder: Send + Sync {
    /// Compute the embedding vector for the given text
    fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>>>;
}
