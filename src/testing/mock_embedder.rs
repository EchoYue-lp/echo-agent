//! 测试用 MockEmbedder
//!
//! 基于文本哈希生成确定性的归一化向量，无需真实 API，适合单元测试和集成测试。

use crate::error::Result;
use crate::memory::embedder::Embedder;
use async_trait::async_trait;

/// 测试用嵌入器，基于字节哈希生成确定性伪嵌入向量
///
/// - 相同文本总是生成相同向量（确定性）
/// - 文本越相似，向量余弦距离越接近（语义感知程度有限，但足够测试流程）
/// - 零网络请求，适合 CI 环境
///
/// # 示例
///
/// ```rust
/// use echo_agent::testing::MockEmbedder;
/// use echo_agent::memory::Embedder;
///
/// # #[tokio::main]
/// # async fn main() {
/// let embedder = MockEmbedder::new(8);
/// let vec = embedder.embed("hello world").await.unwrap();
/// assert_eq!(vec.len(), 8);
/// // 归一化向量，模长约为 1.0
/// let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
/// assert!((norm - 1.0).abs() < 1e-5);
/// # }
/// ```
pub struct MockEmbedder {
    dimension: usize,
}

impl MockEmbedder {
    /// 创建指定维度的 MockEmbedder（推荐维度 4~64，足够测试相似度逻辑）
    pub fn new(dimension: usize) -> Self {
        assert!(dimension > 0, "dimension 必须 > 0");
        Self { dimension }
    }
}

#[async_trait]
impl Embedder for MockEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let mut vec = vec![0.0f32; self.dimension];
        // 用字节值累积到各维度（确定性）
        for (i, b) in text.bytes().enumerate() {
            vec[i % self.dimension] += b as f32;
        }
        // L2 归一化，使余弦相似度有意义
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut vec {
                *v /= norm;
            }
        }
        Ok(vec)
    }
}
