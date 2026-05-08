//! Web extract tool — extract structured content from HTML
//!
//! Extracts title, links, headings, paragraphs, tables from HTML content.

use echo_core::error::{Result, ToolError};
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use scraper::{Html, Selector};
use serde_json::Value;

pub struct WebExtractTool;

impl Tool for WebExtractTool {
    fn name(&self) -> &str {
        "web_extract"
    }

    fn description(&self) -> &str {
        "Extract structured information from HTML content: title, all links, paragraphs, tables, etc."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "html": {
                    "type": "string",
                    "description": "HTML content to parse"
                },
                "extract_type": {
                    "type": "string",
                    "enum": ["links", "headings", "paragraphs", "tables", "all"],
                    "description": "Extraction type (default 'all')"
                }
            },
            "required": ["html"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let html = parameters
                .get("html")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("html".to_string()))?;

            let extract_type = parameters
                .get("extract_type")
                .and_then(|v| v.as_str())
                .unwrap_or("all");

            let document = Html::parse_document(html);
            let mut result = serde_json::json!({});

            if (extract_type == "all" || extract_type == "headings")
                && let Ok(sel) = Selector::parse("h1, h2, h3, h4, h5, h6")
            {
                let headings: Vec<Value> = document
                    .select(&sel)
                    .map(|el| {
                        let tag = el.value().name().to_string();
                        let text = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                        serde_json::json!({"tag": tag, "text": text})
                    })
                    .take(50)
                    .collect();
                if !headings.is_empty() {
                    result["headings"] = serde_json::json!(headings);
                }
            }

            if (extract_type == "all" || extract_type == "links")
                && let Ok(sel) = Selector::parse("a[href]")
            {
                let links: Vec<Value> = document
                    .select(&sel)
                    .filter_map(|el| {
                        let href = el.value().attr("href")?;
                        let text = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                        if href.starts_with('#') || href.is_empty() {
                            None
                        } else {
                            Some(serde_json::json!({"text": text, "href": href}))
                        }
                    })
                    .take(100)
                    .collect();
                if !links.is_empty() {
                    result["links"] = serde_json::json!(links);
                }
            }

            if (extract_type == "all" || extract_type == "paragraphs")
                && let Ok(sel) = Selector::parse("p")
            {
                let paragraphs: Vec<String> = document
                    .select(&sel)
                    .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
                    .filter(|t| !t.is_empty())
                    .take(30)
                    .collect();
                if !paragraphs.is_empty() {
                    result["paragraphs"] = serde_json::json!(paragraphs);
                }
            }

            if (extract_type == "all" || extract_type == "tables")
                && let Ok(sel) = Selector::parse("table")
            {
                let table_count = document.select(&sel).count();
                if table_count > 0 {
                    result["table_count"] = serde_json::json!(table_count);
                }
            }

            if result.as_object().is_none_or(|o| o.is_empty()) {
                Ok(ToolResult::success_json(serde_json::json!({
                    "error": "Could not extract meaningful structured content from the HTML"
                })))
            } else {
                Ok(ToolResult::success_json(result))
            }
        })
    }
}
