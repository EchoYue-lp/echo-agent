//! ArXiv paper search tool.
//!
//! Queries the ArXiv API (Atom/XML) and returns structured paper metadata.

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::Value;
use std::sync::OnceLock;

const TOOL_NAME: &str = "arxiv_search";
const ARXIV_API_URL: &str = "https://export.arxiv.org/api/query";

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

pub struct ArxivSearchTool;

impl Tool for ArxivSearchTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]
    }

    fn description(&self) -> &str {
        "Search arxiv for academic papers. Returns title, authors, abstract, arxiv ID, PDF URL, categories, and publication date. Supports category filtering (e.g. 'cs.AI', 'cs.LG', 'math.CO') and sorting. Max 100 results per query. Example: arxiv_search(query='transformer attention mechanism', max_results=20, category='cs.CL')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (keywords, title, author name, etc.)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results (default 20, max 100)"
                },
                "category": {
                    "type": "string",
                    "description": "ArXiv category filter (optional). Common: cs.AI, cs.LG, cs.CL, cs.CV, math.CO, stat.ML, physics"
                },
                "sort_by": {
                    "type": "string",
                    "description": "Sort order: 'relevance' (default), 'lastUpdatedDate', 'submittedDate'"
                },
                "sort_order": {
                    "type": "string",
                    "description": "Sort direction: 'descending' (default) or 'ascending'"
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

            let max_results = parameters
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(20)
                .min(100) as usize;

            let category = parameters.get("category").and_then(|v| v.as_str());
            let sort_by = parameters
                .get("sort_by")
                .and_then(|v| v.as_str())
                .unwrap_or("relevance");
            let sort_order = parameters
                .get("sort_order")
                .and_then(|v| v.as_str())
                .unwrap_or("descending");

            // Build search query (URL-encode to prevent parameter injection)
            let search_query = if let Some(cat) = category {
                format!(
                    "cat:{}+AND+{}",
                    urlencoding::encode(cat),
                    urlencoding::encode(query)
                )
            } else {
                urlencoding::encode(query).to_string()
            };

            let sort_param = match sort_by {
                "lastUpdatedDate" => "lastUpdatedDate",
                "submittedDate" => "submittedDate",
                _ => "relevance",
            };

            let url = format!(
                "{}?search_query={}&start=0&max_results={}&sortBy={}&sortOrder={}",
                ARXIV_API_URL, search_query, max_results, sort_param, sort_order
            );

            let client = shared_client();

            let response =
                client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("ArXiv API request failed: {}", e),
                    })?;

            let xml_text = response
                .text()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to read ArXiv response: {}", e),
                })?;

            let papers = parse_arxiv_atom(&xml_text)?;

            let result = serde_json::json!({
                "query": query,
                "total_results": papers.len(),
                "papers": papers,
            });

            Ok(ToolResult::success_json(result))
        })
    }
}

/// Parse ArXiv Atom XML response into structured paper records.
fn parse_arxiv_atom(xml: &str) -> Result<Vec<Value>> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut papers = Vec::new();
    let mut in_entry = false;
    let mut current_tag = String::new();

    // Current entry fields
    let mut title = String::new();
    let mut summary = String::new();
    let mut published = String::new();
    let mut arxiv_id = String::new();
    let mut pdf_url = String::new();
    let mut authors: Vec<String> = Vec::new();
    let mut categories: Vec<String> = Vec::new();
    let mut in_author = false;
    let mut author_name = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag.as_str() {
                    "entry" => {
                        in_entry = true;
                        title.clear();
                        summary.clear();
                        published.clear();
                        arxiv_id.clear();
                        pdf_url.clear();
                        authors.clear();
                        categories.clear();
                    }
                    "author" if in_entry => {
                        in_author = true;
                        author_name.clear();
                    }
                    "name" if in_author => {
                        current_tag = "author_name".to_string();
                    }
                    "title" if in_entry => {
                        current_tag = "title".to_string();
                    }
                    "summary" if in_entry => {
                        current_tag = "summary".to_string();
                    }
                    "published" if in_entry => {
                        current_tag = "published".to_string();
                    }
                    "id" if in_entry => {
                        current_tag = "id".to_string();
                    }
                    "link" if in_entry => {
                        // Check for PDF link
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"title" && attr.value.as_ref() == b"pdf" {
                                for attr2 in e.attributes().flatten() {
                                    if attr2.key.as_ref() == b"href" {
                                        pdf_url = String::from_utf8_lossy(&attr2.value).to_string();
                                    }
                                }
                            }
                        }
                    }
                    "category" if in_entry => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"term" {
                                categories.push(String::from_utf8_lossy(&attr.value).to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                match current_tag.as_str() {
                    "title" if in_entry => title = text,
                    "summary" if in_entry => summary = text,
                    "published" if in_entry => published = text,
                    "id" if in_entry => arxiv_id = text,
                    "author_name" if in_author => author_name = text,
                    _ => {}
                }
                current_tag.clear();
            }
            Ok(Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag.as_str() {
                    "entry" => {
                        if in_entry {
                            // Extract arxiv ID from URL
                            let clean_id =
                                arxiv_id.rsplit('/').next().unwrap_or(&arxiv_id).to_string();

                            papers.push(serde_json::json!({
                                "arxiv_id": clean_id,
                                "title": title.trim(),
                                "authors": authors,
                                "abstract": summary.trim(),
                                "published": published,
                                "pdf_url": pdf_url,
                                "categories": categories,
                            }));
                            in_entry = false;
                        }
                    }
                    "author" => {
                        if in_author {
                            if !author_name.is_empty() {
                                authors.push(author_name.clone());
                            }
                            in_author = false;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
    }

    Ok(papers)
}
