//! 复用 SearchProvider 合同的 Exa 搜索后端。

use super::{SearchProvider, SearchResult};
use async_trait::async_trait;
use echo_core::error::Result;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

/// 通过 Exa `/search` 搜索互联网，复用原通用provider合同。
///
/// 只取highlights作为snippet；所选网页的正文仍由独立web_fetch读取。
pub struct ExaSearchProvider {
    client: Client,
    api_key: String,
    endpoint: String,
}

impl ExaSearchProvider {
    /// 使用调用方提供的Exa Key；不改变框架其它凭据与provider配置。
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            api_key: api_key.into(),
            endpoint: "https://api.exa.ai/search".to_string(),
        }
    }

    /// 从 `EXA_API_KEY` 读取非空Key。
    pub fn from_env() -> Option<Self> {
        std::env::var("EXA_API_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty())
            .map(Self::new)
    }
}

#[derive(Deserialize)]
struct ExaResponse {
    results: Vec<ExaResult>,
}

#[derive(Deserialize)]
struct ExaResult {
    title: Option<String>,
    url: Option<String>,
    highlights: Option<Vec<String>>,
}

#[async_trait]
impl SearchProvider for ExaSearchProvider {
    fn name(&self) -> &str {
        "exa"
    }

    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        if max_results == 0 {
            return Ok(Vec::new());
        }
        // provider直调最多100条；工具层仍使用原10条窗口与limit/cursor合同。
        let max_results = max_results.min(100);
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "query": query,
                "type": "auto",
                "numResults": max_results,
                "contents": {"highlights": {"maxCharacters": 1000}}
            }))
            .send()
            .await
            .map_err(|error| crate::http_body::request_error("web_search", error, None))?;

        if !response.status().is_success() {
            // 本地场景也必须避免把密钥或查询经上游错误正文泄漏到模型/日志。
            // 复用统一typed状态/Retry-After，不以展示文字重新分类或更换引擎。
            return Err(crate::http_body::response_error("web_search", &response));
        }

        let response: ExaResponse = crate::http_body::read_bounded_json(
            response,
            crate::http_body::MAX_API_RESPONSE_BYTES,
            "web_search",
            None,
        )
        .await?;

        Ok(response
            .results
            .into_iter()
            .filter_map(|result| {
                let title = result.title?.trim().to_string();
                let url = result.url?.trim().to_string();
                if title.is_empty() || url.is_empty() {
                    return None;
                }
                let highlights = result.highlights.unwrap_or_default();
                let snippet = highlights
                    .iter()
                    .map(|value| value.trim())
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(SearchResult {
                    title,
                    url,
                    snippet,
                })
            })
            .take(max_results)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::providers::SearchProvider;
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    type TestResult = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

    async fn mock_provider(
        status: &str,
        body: &str,
    ) -> std::io::Result<(
        ExaSearchProvider,
        tokio::task::JoinHandle<std::io::Result<(String, Value)>>,
    )> {
        mock_provider_with_headers(status, "", body).await
    }

    async fn mock_provider_with_headers(
        status: &str,
        extra_headers: &str,
        body: &str,
    ) -> std::io::Result<(
        ExaSearchProvider,
        tokio::task::JoinHandle<std::io::Result<(String, Value)>>,
    )> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",
            body.len()
        );
        let server = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(5), async move {
                let (mut socket, _) = listener.accept().await?;
                let mut request = Vec::new();
                let (headers, body) = loop {
                    let mut chunk = [0_u8; 4096];
                    let count = socket.read(&mut chunk).await?;
                    if count == 0 || request.len() > 64 * 1024 {
                        return Err(std::io::Error::other("incomplete or oversized request"));
                    }
                    request.extend_from_slice(chunk.get(..count).unwrap_or_default());
                    if let Some(offset) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                        let body_offset = offset.saturating_add(4);
                        let headers =
                            String::from_utf8_lossy(request.get(..offset).unwrap_or_default());
                        let content_length = headers
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>())
                            .transpose()
                            .map_err(std::io::Error::other)?
                            .unwrap_or_default();
                        if request.len() >= body_offset.saturating_add(content_length) {
                            let body = serde_json::from_slice(
                                request.get(body_offset..).unwrap_or_default(),
                            )?;
                            break (headers.into_owned(), body);
                        }
                    }
                };
                socket.write_all(response.as_bytes()).await?;
                Ok((headers, body))
            })
            .await
            .map_err(std::io::Error::other)?
        });
        let mut provider = ExaSearchProvider::new("test-exa-key");
        // 本机HTTP夹具独立于开发环境代理。
        provider.client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(std::io::Error::other)?;
        provider.endpoint = format!("http://{address}/search");
        Ok((provider, server))
    }

    #[tokio::test]
    async fn sends_search_contract_and_normalizes_highlights() -> TestResult {
        let body = json!({"results": [
            {"title": " 中文搜索 🦀 ", "url": " https://example.com/rust ",
             "highlights": [" 摘要一 ", "", " 摘要二 🦀 "]},
            {"title": "Missing URL"},
            {"title": " ", "url": "https://example.com/invalid"},
            {"title": "No highlights", "url": "https://example.com/empty"},
            {"title": "Beyond limit", "url": "https://example.com/extra"}
        ]})
        .to_string();
        let (provider, server) = mock_provider("200 OK", &body).await?;
        let results = provider.search("Rust 中文 🦀", 2).await?;
        let (headers, request) = server.await??;
        assert!(headers.starts_with("POST /search HTTP/1.1"));
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("authorization: bearer test-exa-key")
        );
        assert_eq!(
            request,
            json!({
                "query": "Rust 中文 🦀", "type": "auto", "numResults": 2,
                "contents": {"highlights": {"maxCharacters": 1000}}
            })
        );
        assert_eq!(results.len(), 2);
        let first = results.first().ok_or("missing first result")?;
        assert_eq!(first.title, "中文搜索 🦀");
        assert_eq!(first.url, "https://example.com/rust");
        assert_eq!(first.snippet, "摘要一\n摘要二 🦀");
        assert_eq!(results.get(1).ok_or("missing second result")?.snippet, "");
        Ok(())
    }

    #[tokio::test]
    async fn accepts_empty_results_but_rejects_malformed_success() -> TestResult {
        for (body, success) in [
            (r#"{"results": []}"#, true),
            ("{}", false),
            ("invalid", false),
        ] {
            let (provider, server) = mock_provider("200 OK", body).await?;
            let result = provider.search("rust", 5).await;
            server.await??;
            assert_eq!(result.is_ok(), success, "body: {body}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn web_search_preserves_results_and_cursor_contract() -> TestResult {
        use crate::web::WebSearchTool;
        use echo_core::tools::{Tool, ToolParameters};

        let body = json!({"results": [
            {"title": "第一条", "url": "https://example.com/first", "highlights": ["摘要"]},
            {"title": "第二条", "url": "https://example.com/second", "highlights": null}
        ]})
        .to_string();
        let mut parameters = ToolParameters::new();
        parameters.insert("query".to_string(), json!("中文"));
        parameters.insert("limit".to_string(), json!(1));
        for (index, title) in ["第一条", "第二条"].into_iter().enumerate() {
            let (provider, server) = mock_provider("200 OK", &body).await?;
            let result = WebSearchTool::new(Box::new(provider))
                .execute(parameters.clone())
                .await?;
            let (_, request) = server.await??;
            assert!(result.success);
            assert_eq!(request.get("numResults"), Some(&json!(10)));
            let entries = result
                .data
                .as_ref()
                .and_then(Value::as_array)
                .ok_or("missing structured results")?;
            assert_eq!(entries.len(), 1);
            assert_eq!(
                entries.first().and_then(|entry| entry.get("title")),
                Some(&json!(title))
            );
            if index == 0 {
                let cursor = result
                    .metadata
                    .get("page.next_cursor")
                    .ok_or("missing cursor")?;
                parameters.insert("cursor".to_string(), json!(cursor));
                assert!(result.truncated);
            } else {
                assert!(!result.truncated);
                assert!(!result.metadata.contains_key("page.next_cursor"));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn reports_http_failures_without_echoing_sensitive_response() -> TestResult {
        use echo_core::error::{ReactError, ToolError};
        use echo_core::tools::{Tool, ToolFailure, ToolFailureCategory, ToolParameters};
        for (status, code, category, retryable) in [
            (
                "401 Unauthorized",
                401,
                ToolFailureCategory::Permanent,
                false,
            ),
            (
                "402 Payment Required",
                402,
                ToolFailureCategory::Permanent,
                false,
            ),
            (
                "429 Too Many Requests",
                429,
                ToolFailureCategory::Transient,
                true,
            ),
            (
                "500 Internal Server Error",
                500,
                ToolFailureCategory::Transient,
                true,
            ),
        ] {
            let (provider, server) =
                mock_provider_with_headers(status, "Retry-After: 2\r\n", "secret-test-exa-key")
                    .await?;
            let error = provider
                .search("rust", 5)
                .await
                .err()
                .ok_or("expected failure")?;
            server.await??;
            assert!(matches!(&error, ReactError::Tool(error)
                if matches!(error.as_ref(), ToolError::RemoteFailure { http_status: Some(value), .. } if *value == code)));
            let failure = ToolFailure::from_error(&error, false);
            assert_eq!(failure.category, category);
            assert_eq!(failure.allows_automatic_retry(), retryable);
            assert_eq!(failure.retry_after_ms, Some(2_000));
            let message = error.to_string();
            assert!(message.contains(&code.to_string()), "{message}");
            assert!(!message.contains("test-exa-key"));

            // 同一真实HTTP失败再穿过WebSearchTool，不能丢失typed分类/等待提示/status。
            let (provider, server) =
                mock_provider_with_headers(status, "Retry-After: 2\r\n", "secret-test-exa-key")
                    .await?;
            let mut parameters = ToolParameters::new();
            parameters.insert("query".into(), json!("rust"));
            let result = crate::web::WebSearchTool::new(Box::new(provider))
                .execute(parameters)
                .await?;
            server.await??;
            assert!(!result.success);
            let failure = result.failure.ok_or("missing typed tool failure")?;
            assert_eq!(failure.category, category);
            assert_eq!(failure.allows_automatic_retry(), retryable);
            assert_eq!(failure.retry_after_ms, Some(2_000));
            assert_eq!(result.metadata.get("http_status"), Some(&code.to_string()));
            assert!(!result.error.unwrap_or_default().contains("test-exa-key"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn bounds_provider_result_count() -> TestResult {
        let (provider, server) = mock_provider("200 OK", r#"{"results": []}"#).await?;
        provider.search("rust", usize::MAX).await?;
        let (_, request) = server.await??;
        assert_eq!(request.get("numResults"), Some(&json!(100)));
        assert!(provider.search("rust", 0).await?.is_empty());
        Ok(())
    }
}
