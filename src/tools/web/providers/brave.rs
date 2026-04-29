//! Brave Search API Provider
//!
//! 通过 Brave Search API 实现高质量搜索，需要 API Key。
//!
//! # 获取 API Key
//!
//! 前往 <https://brave.com/search/api/> 注册获取免费 API Key（每月 2000 次免费额度）。

use super::utils::{truncate_chars, urlencode};
use super::{SearchProvider, SearchResult};
use crate::error::{Result, ToolError};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

/// Brave Search API Provider
///
/// 使用 Brave Search API 进行搜索，需要 API Key。
pub struct BraveSearchProvider {
    client: Client,
    api_key: String,
}

impl BraveSearchProvider {
    /// 创建新的 Brave Search Provider
    ///
    /// - `api_key`: Brave Search API Key
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            api_key: api_key.into(),
        }
    }

    /// 从环境变量创建
    ///
    /// 读取 `BRAVE_SEARCH_API_KEY` 环境变量。
    pub fn from_env() -> Option<Self> {
        std::env::var("BRAVE_SEARCH_API_KEY").ok().map(Self::new)
    }
}

/// Brave Search API 响应结构
#[derive(Debug, Deserialize)]
struct BraveResponse {
    web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    results: Option<Vec<BraveResult>>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    title: Option<String>,
    url: Option<String>,
    description: Option<String>,
}

#[async_trait]
impl SearchProvider for BraveSearchProvider {
    fn name(&self) -> &str {
        "brave"
    }

    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencode(query),
            max_results
        );

        let response = self
            .client
            .get(&url)
            .header("X-Subscription-Token", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: format!("Brave Search 请求失败: {}", e),
            })?;

        let status = response.status();
        if status.as_u16() == 401 {
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: "Brave Search API Key 无效或已过期".into(),
            }
            .into());
        }
        if status.as_u16() == 429 {
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: "Brave Search API 调用次数已达上限".into(),
            }
            .into());
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: format!(
                    "Brave Search 返回错误 ({}): {}",
                    status,
                    truncate_chars(&body, 200)
                ),
            }
            .into());
        }

        let brave_resp: BraveResponse =
            response
                .json()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "web_search".into(),
                    message: format!("Brave Search 响应解析失败: {}", e),
                })?;

        let results = brave_resp.web.and_then(|w| w.results).unwrap_or_default();

        Ok(results
            .into_iter()
            .take(max_results)
            .filter_map(|r| {
                let title = r.title?.trim().to_string();
                let url = r.url?.trim().to_string();
                if title.is_empty() || url.is_empty() {
                    return None;
                }
                Some(SearchResult {
                    title,
                    url,
                    snippet: r.description.unwrap_or_default().trim().to_string(),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencode() {
        assert_eq!(urlencode("hello world"), "hello%20world");
        assert_eq!(urlencode("rust-lang"), "rust-lang");
        assert_eq!(urlencode("你好"), "%E4%BD%A0%E5%A5%BD");
    }

    #[test]
    fn test_parse_brave_response() {
        let json = r#"{
            "web": {
                "results": [
                    {
                        "title": "Rust Programming Language",
                        "url": "https://www.rust-lang.org/",
                        "description": "A language empowering everyone"
                    },
                    {
                        "title": "Rust Documentation",
                        "url": "https://doc.rust-lang.org/",
                        "description": null
                    }
                ]
            }
        }"#;

        let resp: BraveResponse = serde_json::from_str(json).unwrap();
        let results: Vec<SearchResult> = resp
            .web
            .and_then(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| {
                let title = r.title?.trim().to_string();
                let url = r.url?.trim().to_string();
                if title.is_empty() || url.is_empty() {
                    return None;
                }
                Some(SearchResult {
                    title,
                    url,
                    snippet: r.description.unwrap_or_default().trim().to_string(),
                })
            })
            .collect();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].snippet, "A language empowering everyone");
        assert!(results[1].snippet.is_empty());
    }

    #[test]
    fn test_parse_brave_empty_response() {
        let json = r#"{"web": {"results": []}}"#;
        let resp: BraveResponse = serde_json::from_str(json).unwrap();
        let results = resp.web.and_then(|w| w.results).unwrap_or_default();
        assert!(results.is_empty());
    }
}
