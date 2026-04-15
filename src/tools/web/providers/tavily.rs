//! Tavily Search API Provider
//!
//! 通过 Tavily AI 搜索 API 实现 AI 优化搜索，需要 API Key。
//!
//! # 特点
//!
//! - AI 优化的搜索结果，摘要质量更高
//! - 支持自动提取网页内容
//! - 专为 AI Agent 设计
//!
//! # 获取 API Key
//!
//! 前往 <https://tavily.com/> 注册获取 API Key。

use super::utils::truncate_chars;
use super::{SearchProvider, SearchResult};
use crate::error::{Result, ToolError};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Tavily Search API Provider
///
/// 使用 Tavily AI 搜索 API 进行搜索，需要 API Key。
pub struct TavilyProvider {
    client: Client,
    api_key: String,
}

impl TavilyProvider {
    /// 创建新的 Tavily Provider
    ///
    /// - `api_key`: Tavily API Key
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("Failed to build reqwest client");
        Self {
            client,
            api_key: api_key.into(),
        }
    }

    /// 从环境变量创建
    ///
    /// 读取 `TAVILY_API_KEY` 环境变量。
    pub fn from_env() -> Option<Self> {
        std::env::var("TAVILY_API_KEY").ok().map(Self::new)
    }
}

/// Tavily 搜索请求体
#[derive(Debug, Serialize)]
struct TavilyRequest {
    api_key: String,
    query: String,
    max_results: usize,
    #[serde(rename = "include_answer")]
    include_answer: bool,
}

/// Tavily 搜索响应
#[derive(Debug, Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
    #[allow(dead_code)]
    answer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    fn name(&self) -> &str {
        "tavily"
    }

    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let request = TavilyRequest {
            api_key: self.api_key.clone(),
            query: query.to_string(),
            max_results,
            include_answer: false,
        };

        let response = self
            .client
            .post("https://api.tavily.com/search")
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: format!("Tavily 请求失败: {}", e),
            })?;

        let status = response.status();
        if status.as_u16() == 401 {
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: "Tavily API Key 无效或已过期".into(),
            }
            .into());
        }
        if status.as_u16() == 429 {
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: "Tavily API 调用次数已达上限".into(),
            }
            .into());
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: format!(
                    "Tavily 返回错误 ({}): {}",
                    status,
                    truncate_chars(&body, 200)
                ),
            }
            .into());
        }

        let tavily_resp: TavilyResponse =
            response
                .json()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "web_search".into(),
                    message: format!("Tavily 响应解析失败: {}", e),
                })?;

        Ok(tavily_resp
            .results
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
                    snippet: r.content.unwrap_or_default().trim().to_string(),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tavily_request_serialization() {
        let req = TavilyRequest {
            api_key: "test-key".into(),
            query: "rust programming".into(),
            max_results: 5,
            include_answer: false,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["api_key"], "test-key");
        assert_eq!(json["query"], "rust programming");
        assert_eq!(json["max_results"], 5);
        assert_eq!(json["include_answer"], false);
    }

    #[test]
    fn test_parse_tavily_response() {
        let json = r#"{
            "results": [
                {
                    "title": "Rust Programming Language",
                    "url": "https://www.rust-lang.org/",
                    "content": "A language empowering everyone to build reliable and efficient software."
                },
                {
                    "title": "Learn Rust",
                    "url": "https://doc.rust-lang.org/book/",
                    "content": "The Rust Programming Language book."
                }
            ],
            "answer": "Rust is a systems programming language."
        }"#;

        let resp: TavilyResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.results.len(), 2);
        assert!(resp.answer.is_some());

        let results: Vec<SearchResult> = resp
            .results
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
                    snippet: r.content.unwrap_or_default().trim().to_string(),
                })
            })
            .collect();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(
            results[0].snippet,
            "A language empowering everyone to build reliable and efficient software."
        );
    }

    #[test]
    fn test_parse_tavily_empty_response() {
        let json = r#"{"results": [], "answer": null}"#;
        let resp: TavilyResponse = serde_json::from_str(json).unwrap();
        assert!(resp.results.is_empty());
        assert!(resp.answer.is_none());
    }
}
