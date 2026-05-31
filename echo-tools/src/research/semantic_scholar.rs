//! Semantic Scholar paper search tool.
//!
//! Queries the Semantic Scholar API for academic papers with rich metadata
//! (citation counts, references, fields of study).

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::Value;
use std::sync::OnceLock;

const TOOL_NAME: &str = "semantic_scholar_search";
const S2_API_URL: &str = "https://api.semanticscholar.org/graph/v1";
// Rate limit: 100 requests per 5 minutes without API key
const DEFAULT_FIELDS: &str = "title,abstract,authors,citationCount,referenceCount,year,url,externalIds,fieldsOfStudy,venue,publicationDate";

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn shared_client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(crate::security::ssrf_safe_redirect_policy())
            .build()
            .unwrap_or_default()
    })
}

pub struct SemanticScholarSearchTool;

impl Tool for SemanticScholarSearchTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]
    }

    fn description(&self) -> &str {
        "Search Semantic Scholar for academic papers. Returns rich metadata including citation count, references, fields of study. No API key required (rate-limited). Supports keyword search and paper ID lookup. Example: semantic_scholar_search(query='large language models', limit=20)"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (keywords, title, author)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results (default 20, max 100)"
                },
                "fields": {
                    "type": "string",
                    "description": "Comma-separated fields to return (optional). Default includes title, abstract, authors, citationCount, year, url, fieldsOfStudy, venue"
                },
                "year_range": {
                    "type": "string",
                    "description": "Year range filter (optional). Format: '2020-2025' or '2020-' or '-2023'"
                },
                "fields_of_study": {
                    "type": "string",
                    "description": "Filter by field of study (optional). E.g. 'Computer Science', 'Mathematics'"
                },
                "sort": {
                    "type": "string",
                    "description": "Sort results: 'relevance' (default), 'citationCount:desc', 'year:desc', 'year:asc'"
                }
            },
            "required": ["query"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let query = parameters
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("query".to_string()))?;

            let limit = parameters
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(20)
                .min(100) as usize;

            let fields = parameters
                .get("fields")
                .and_then(|v| v.as_str())
                .unwrap_or(DEFAULT_FIELDS);

            let year_range = parameters.get("year_range").and_then(|v| v.as_str());
            let fields_of_study = parameters.get("fields_of_study").and_then(|v| v.as_str());
            let sort = parameters.get("sort").and_then(|v| v.as_str());

            // Build URL
            let mut url = format!(
                "{}/paper/search?query={}&limit={}&fields={}",
                S2_API_URL,
                urlencoding::encode(query),
                limit,
                fields
            );

            if let Some(yr) = year_range {
                url.push_str(&format!("&year={}", yr));
            }
            if let Some(fos) = fields_of_study {
                url.push_str(&format!("&fieldsOfStudy={}", urlencoding::encode(fos)));
            }
            if let Some(s) = sort {
                url.push_str(&format!("&sort={}", s));
            }

            let client = shared_client();

            let response =
                client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("Semantic Scholar API request failed: {}", e),
                    })?;

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Semantic Scholar API error ({}): {}", status, body),
                }
                .into());
            }

            let json: Value = response
                .json()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to parse Semantic Scholar response: {}", e),
                })?;

            let total = json.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
            let papers = json
                .get("data")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            // Normalize paper format
            let normalized: Vec<Value> = papers
                .iter()
                .map(|p| {
                    let paper_id = p.get("paperId").and_then(|v| v.as_str()).unwrap_or("");
                    let external_ids = p.get("externalIds");
                    let arxiv_id = external_ids
                        .and_then(|ids| ids.get("ArXiv"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let doi = external_ids
                        .and_then(|ids| ids.get("DOI"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    serde_json::json!({
                        "paper_id": paper_id,
                        "title": p.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                        "authors": p.get("authors")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter()
                                .map(|a| a.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string())
                                .collect::<Vec<_>>())
                            .unwrap_or_default(),
                        "abstract": p.get("abstract").and_then(|v| v.as_str()).unwrap_or(""),
                        "year": p.get("year"),
                        "citation_count": p.get("citationCount"),
                        "reference_count": p.get("referenceCount"),
                        "venue": p.get("venue").and_then(|v| v.as_str()).unwrap_or(""),
                        "url": p.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                        "arxiv_id": arxiv_id,
                        "doi": doi,
                        "fields_of_study": p.get("fieldsOfStudy")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter()
                                .map(|f| f.as_str().unwrap_or("").to_string())
                                .collect::<Vec<_>>())
                            .unwrap_or_default(),
                        "publication_date": p.get("publicationDate").and_then(|v| v.as_str()).unwrap_or(""),
                    })
                })
                .collect();

            let result = serde_json::json!({
                "query": query,
                "total_available": total,
                "returned": normalized.len(),
                "papers": normalized,
            });

            Ok(ToolResult::success_json(result))
        })
    }
}
