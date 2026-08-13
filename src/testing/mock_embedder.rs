//! MockEmbedder for testing
//!
//! Generates deterministic normalized vectors based on text hashing, requires
//! no real API, suitable for unit tests and integration tests.

use crate::error::Result;
use crate::memory::embedder::Embedder;
use futures::future::BoxFuture;

/// Test embedder that generates deterministic pseudo-embedding vectors based on byte hashing
///
/// - Same text always produces the same vector (deterministic)
/// - More similar text yields closer cosine distance (limited semantic awareness, but sufficient for testing flows)
/// - Zero network requests, suitable for CI environments
///
/// # Example
///
/// ```rust
/// use echo_agent::testing::MockEmbedder;
/// use echo_agent::memory::Embedder;
///
/// # #[tokio::main]
/// # async fn main() {
/// let embedder = MockEmbedder::new(8).unwrap();
/// let vec = embedder.embed("hello world").await.unwrap();
/// assert_eq!(vec.len(), 8);
/// // Normalized vector, magnitude approximately 1.0
/// let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
/// assert!((norm - 1.0).abs() < 1e-5);
/// # }
/// ```
pub struct MockEmbedder {
    dimension: usize,
}

impl MockEmbedder {
    /// Create a MockEmbedder with the specified dimension (recommended 4~64, sufficient for testing similarity logic)
    pub fn new(dimension: usize) -> Result<Self> {
        if dimension == 0 {
            return Err(crate::error::ReactError::Other(
                "mock embedder dimension must be greater than zero".to_string(),
            ));
        }
        Ok(Self { dimension })
    }
}

impl Embedder for MockEmbedder {
    fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>>> {
        Box::pin(async move {
            let mut vec = vec![0.0f32; self.dimension];
            // Accumulate byte values into each dimension (deterministic)
            for (i, b) in text.bytes().enumerate() {
                vec[i % self.dimension] += b as f32;
            }
            // L2 normalization so cosine similarity is meaningful
            let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for v in &mut vec {
                    *v /= norm;
                }
            }
            Ok(vec)
        })
    }
}
