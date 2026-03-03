//! 文本嵌入接口
//!
//! 将文本映射为稠密浮点向量，供 [`EmbeddingStore`](super::EmbeddingStore) 做语义检索。

use crate::error::{MemoryError, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::debug;

// ── Embedder trait ────────────────────────────────────────────────────────────

/// 文本嵌入接口：将文本映射为稠密浮点向量
#[async_trait]
pub trait Embedder: Send + Sync {
    /// 计算文本的嵌入向量
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

// ── HTTP 嵌入客户端（OpenAI 兼容接口）────────────────────────────────────────

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedData>,
}

#[derive(Deserialize)]
struct EmbedData {
    embedding: Vec<f32>,
}

/// OpenAI 兼容的 HTTP 嵌入客户端
///
/// 支持 OpenAI、Qwen（DashScope）等兼容 `/v1/embeddings` 接口的服务。
///
/// > ⚠️ **注意**：必须使用支持 Embedding 的模型（如 `text-embedding-v3`、
/// > `text-embedding-3-small`）。对话模型（ChatGPT、DeepSeek-Chat 等）
/// > 不提供嵌入接口，配置错误会在运行时报 API 错误。
///
/// # 环境变量
///
/// 支持两套命名风格，优先级从高到低：
///
/// | 用途 | 优先读取 | 备选 | 最终备选 |
/// |------|----------|------|--------|
/// | 完整端点 URL | `EMBEDDING_BASEURL` | — | — |
/// | 基础 URL（自动拼 `/v1/embeddings`）| `EMBEDDING_API_URL` | — | `https://api.openai.com` |
/// | API 密钥 | `EMBEDDING_APIKEY` | `EMBEDDING_API_KEY` | `OPENAI_API_KEY` |
/// | 模型名称 | `EMBEDDING_MODEL` | — | `text-embedding-3-small` |
///
/// > `EMBEDDING_BASEURL` 与 `EMBEDDING_API_URL` 二选一：
/// > - `EMBEDDING_BASEURL`：完整 URL，直接使用（如 `https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings`）
/// > - `EMBEDDING_API_URL`：仅 base（如 `https://dashscope.aliyuncs.com/compatible-mode`），代码自动追加 `/v1/embeddings`
///
/// # 示例
///
/// ```rust,no_run
/// use echo_agent::memory::HttpEmbedder;
///
/// // 从环境变量自动读取
/// let embedder = HttpEmbedder::from_env();
///
/// // 显式指定 base URL（自动追加 /v1/embeddings）
/// let embedder = HttpEmbedder::new(
///     "https://dashscope.aliyuncs.com/compatible-mode",
///     std::env::var("DASHSCOPE_API_KEY").unwrap_or_default(),
///     "text-embedding-v3",
/// );
///
/// // 显式指定完整端点 URL
/// let embedder = HttpEmbedder::with_endpoint(
///     "https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings",
///     std::env::var("DASHSCOPE_API_KEY").unwrap_or_default(),
///     "text-embedding-v3",
/// );
/// ```
pub struct HttpEmbedder {
    client: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
}

impl HttpEmbedder {
    /// 基础 URL 构造：自动在末尾追加 `/v1/embeddings`
    ///
    /// `api_url` 传入不含路径的基础 URL，如 `https://api.openai.com`
    pub fn new(
        api_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let base = api_url.into();
        let base = base.trim_end_matches('/').to_string();
        Self {
            client: reqwest::Client::new(),
            url: format!("{base}/v1/embeddings"),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// 完整端点 URL 构造：直接使用传入的 URL，不追加任何路径
    ///
    /// 适用于已知完整端点地址的场景，如 `https://xxx.com/v1/embeddings`
    pub fn with_endpoint(
        url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.into(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// 从环境变量构建
    ///
    /// 同时兼容两套命名风格（`EMBEDDING_BASEURL` / `EMBEDDING_APIKEY` 和
    /// `EMBEDDING_API_URL` / `EMBEDDING_API_KEY`）。
    pub fn from_env() -> Self {
        // ── URL ───────────────────────────────────────────────────────────────
        // EMBEDDING_BASEURL：完整端点，直接使用
        // EMBEDDING_API_URL：base URL，自动追加 /v1/embeddings
        let (url, is_full_url) = if let Ok(u) = std::env::var("EMBEDDING_BASEURL") {
            (u, true)
        } else {
            let base = std::env::var("EMBEDDING_API_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string());
            (base, false)
        };

        // ── API Key ───────────────────────────────────────────────────────────
        let api_key = std::env::var("EMBEDDING_APIKEY")
            .or_else(|_| std::env::var("EMBEDDING_API_KEY"))
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .unwrap_or_default();

        // ── 模型 ──────────────────────────────────────────────────────────────
        let model = std::env::var("EMBEDDING_MODEL")
            .unwrap_or_else(|_| "text-embedding-3-small".to_string());

        if is_full_url {
            Self::with_endpoint(url, api_key, model)
        } else {
            Self::new(url, api_key, model)
        }
    }
}

#[async_trait]
impl Embedder for HttpEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        debug!(model = %self.model, chars = text.len(), "🔢 计算文本嵌入");
        let req = EmbedRequest {
            model: &self.model,
            input: text,
        };
        let resp = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(&req)
            .send()
            .await
            .map_err(|e| MemoryError::IoError(format!("嵌入请求失败: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(MemoryError::IoError(format!("嵌入 API 错误 {status}: {body}")).into());
        }

        let body: EmbedResponse = resp
            .json()
            .await
            .map_err(|e| MemoryError::SerializationError(format!("嵌入响应解析失败: {e}")))?;

        body.data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| MemoryError::IoError("嵌入 API 返回空结果".to_string()).into())
    }
}
