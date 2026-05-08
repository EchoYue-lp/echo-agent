//! HTTP embedding client (OpenAI-compatible)
//!
//! The [`Embedder`] trait is defined in [`echo_core::memory::embedder`].
//! This module provides the [`HttpEmbedder`] implementation.

use echo_core::error::{MemoryError, Result};
pub use echo_core::memory::embedder::Embedder;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::debug;

// ── HTTP embedding client ────────────────────────────────────────────────────

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

/// OpenAI-compatible HTTP embedding client
///
/// Supports OpenAI, Qwen (DashScope), and other services compatible with the `/v1/embeddings` interface.
///
/// # Environment variables
///
/// | Purpose | Priority 1 | Priority 2 | Priority 3 |
/// |---------|------------|------------|-------------|
/// | Full endpoint URL | `EMBEDDING_BASEURL` | — | — |
/// | Base URL (auto-append `/v1/embeddings`) | `EMBEDDING_API_URL` | — | `https://api.openai.com` |
/// | API key | `EMBEDDING_APIKEY` | `EMBEDDING_API_KEY` | `OPENAI_API_KEY` |
/// | Model name | `EMBEDDING_MODEL` | — | `text-embedding-3-small` |
pub struct HttpEmbedder {
    client: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
    /// Request timeout (default 30s)
    timeout: Duration,
}

impl HttpEmbedder {
    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

    /// Base URL constructor: auto-appends `/v1/embeddings`
    pub fn new(
        api_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let base = api_url.into();
        let base = base.trim_end_matches('/').to_string();
        Self {
            client: reqwest::ClientBuilder::new()
                .timeout(Self::DEFAULT_TIMEOUT)
                .build()
                .unwrap_or_default(),
            url: format!("{base}/v1/embeddings"),
            api_key: api_key.into(),
            model: model.into(),
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    /// Full endpoint URL constructor: uses the URL as-is
    pub fn with_endpoint(
        url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client: reqwest::ClientBuilder::new()
                .timeout(Self::DEFAULT_TIMEOUT)
                .build()
                .unwrap_or_default(),
            url: url.into(),
            api_key: api_key.into(),
            model: model.into(),
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    /// Set request timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self.client = reqwest::ClientBuilder::new()
            .timeout(timeout)
            .build()
            .unwrap_or_default();
        self
    }

    /// Construct from environment variables
    pub fn from_env() -> Self {
        let (url, is_full_url) = if let Ok(u) = std::env::var("EMBEDDING_BASEURL") {
            (u, true)
        } else {
            let base = std::env::var("EMBEDDING_API_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string());
            (base, false)
        };

        let api_key = std::env::var("EMBEDDING_APIKEY")
            .or_else(|_| std::env::var("EMBEDDING_API_KEY"))
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .unwrap_or_default();

        let model = std::env::var("EMBEDDING_MODEL")
            .unwrap_or_else(|_| "text-embedding-3-small".to_string());

        if is_full_url {
            Self::with_endpoint(url, api_key, model)
        } else {
            Self::new(url, api_key, model)
        }
        .with_timeout(
            std::env::var("EMBEDDING_TIMEOUT")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map(Duration::from_secs)
                .unwrap_or(Self::DEFAULT_TIMEOUT),
        )
    }
}

impl Embedder for HttpEmbedder {
    fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>>> {
        Box::pin(async move {
            debug!(model = %self.model, chars = text.len(), "Computing text embedding");
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
                .map_err(|e| {
                    echo_core::error::ReactError::from(MemoryError::IoError(e.to_string()))
                })?;

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(
                    MemoryError::IoError(format!("Embedding API error {status}: {body}")).into(),
                );
            }

            let body: EmbedResponse = resp.json().await.map_err(|e| {
                echo_core::error::ReactError::from(MemoryError::SerializationError(format!(
                    "Failed to parse embedding response: {e}"
                )))
            })?;

            body.data
                .into_iter()
                .next()
                .map(|d| d.embedding)
                .ok_or_else(|| {
                    MemoryError::IoError("Embedding API returned empty result".to_string()).into()
                })
        })
    }
}
