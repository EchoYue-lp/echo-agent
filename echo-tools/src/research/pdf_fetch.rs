//! PDF download and parse tool.
//!
//! Downloads a PDF from a URL and extracts text content.
//! Bridges the gap between web fetch (HTML only) and local PDF parsing.

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::Value;

const TOOL_NAME: &str = "pdf_fetch";
const MAX_PDF_BYTES: usize = 50 * 1024 * 1024;
const MAX_EXTRACTED_CHARS: usize = 2_000_000;

pub struct PdfFetchTool;

impl Tool for PdfFetchTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]
    }

    fn description(&self) -> &str {
        "Download a PDF from a URL and extract its text content. Supports arxiv PDF links, direct URLs, and redirects. Useful for reading academic papers. Example: pdf_fetch(url='https://arxiv.org/pdf/1706.03762', pages='1-10')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL of the PDF to download (e.g. https://arxiv.org/pdf/1706.03762)"
                },
                "pages": {
                    "type": "string",
                    "description": "Page range to extract (optional). Examples: '1-5', '1,3,7', 'all'. Default: first 20 pages"
                },
                "max_chars": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 2000000,
                    "description": "Maximum characters to return (default 50000)"
                }
            },
            "required": ["url"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        context: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let url = parameters
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("url".to_string()))?;

            let pages_spec = parameters
                .get("pages")
                .and_then(|v| v.as_str())
                .unwrap_or("1-20");

            let max_chars = parameters
                .get("max_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(50_000);
            let max_chars = usize::try_from(max_chars)
                .unwrap_or(MAX_EXTRACTED_CHARS)
                .clamp(1, MAX_EXTRACTED_CHARS);

            // Download PDF (SSRF-safe: resolve + validate + connect on pinned IPs,
            // closing the DNS-rebinding TOCTOU window)
            let response =
                crate::security::local_http_get(url, std::time::Duration::from_secs(60), 5)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("Failed to download PDF: {}", e),
                    })?;

            if !response.status().is_success() {
                return Err(ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("HTTP error: {}", response.status()),
                }
                .into());
            }

            let bytes = crate::http_body::read_bounded_body(
                response,
                MAX_PDF_BYTES,
                TOOL_NAME,
                Some(context),
            )
            .await?;

            // Verify it's a PDF
            if bytes.get(..4) != Some(b"%PDF".as_slice()) {
                return Err(ToolError::InvalidParameter {
                    name: "url".to_string(),
                    message: "Response is not a PDF file".to_string(),
                }
                .into());
            }

            // Parse PDF from memory (single parse for both text and metadata)
            let doc =
                lopdf::Document::load_mem(&bytes).map_err(|e| ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to parse PDF: {}", e),
                })?;

            let text = extract_pdf_text_from_doc(&doc, pages_spec)?;
            let metadata = extract_pdf_metadata_from_doc(&doc);

            let complete_result = serde_json::json!({
                "url": url,
                "pages_requested": pages_spec,
                "text_length": text.chars().count(),
                "text": text,
                "metadata": metadata,
            });
            if text.chars().count() <= max_chars {
                return Ok(ToolResult::success_json(complete_result));
            }

            let complete_output = serde_json::to_string(&complete_result).map_err(|error| {
                ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("Failed to serialize PDF result: {error}"),
                }
            })?;
            let identity = echo_core::tools::artifact::ToolOutputArtifactIdentity::from_context(
                context, TOOL_NAME,
            );
            let config = context
                .output_artifacts
                .clone()
                .unwrap_or_default()
                .threshold_bytes(1);
            let artifact =
                echo_core::tools::artifact::persist_tool_output(config, identity, &complete_output)
                    .map_err(|error| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("Failed to persist complete PDF result: {error}"),
                    })?
                    .ok_or_else(|| {
                        ToolError::ExecutionFailed {
                tool: TOOL_NAME.to_string(),
                message: "Complete PDF result exceeded the inline limit but no artifact was created"
                    .to_string(),
            }
                    })?;

            let preview: String = text.chars().take(max_chars).collect();
            let preview_result = serde_json::json!({
                "url": url,
                "pages_requested": pages_spec,
                "text_length": text.chars().count(),
                "text": preview,
                "text_complete": false,
                "metadata": metadata,
            });
            let mut result = ToolResult::success_json(preview_result).with_truncated(true);
            artifact.extend_metadata(&mut result.metadata);
            result.output.push_str(&format!(
                "\n\n[PDF text preview only. Full result artifact: {}. Use read_artifact with expected_sha256 {}.]",
                artifact.path.display(),
                artifact.sha256
            ));

            Ok(result)
        })
    }
}

/// Extract text from a parsed PDF Document.
fn extract_pdf_text_from_doc(doc: &lopdf::Document, pages_spec: &str) -> Result<String> {
    let page_count =
        u32::try_from(doc.get_pages().len()).map_err(|_| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: "PDF page count exceeds the supported range".to_string(),
        })?;
    let page_numbers = parse_page_range(pages_spec, page_count)?;

    let mut all_text = String::new();
    let mut extracted_chars = 0usize;

    for page_num in page_numbers {
        match doc.extract_text(&[page_num]) {
            Ok(text) => {
                if !text.trim().is_empty() {
                    let additional_chars =
                        text.chars()
                            .count()
                            .checked_add(32)
                            .ok_or(ToolError::FileTooLarge {
                                size: u64::MAX,
                                max: MAX_EXTRACTED_CHARS as u64,
                            })?;
                    extracted_chars = extracted_chars.checked_add(additional_chars).ok_or(
                        ToolError::FileTooLarge {
                            size: u64::MAX,
                            max: MAX_EXTRACTED_CHARS as u64,
                        },
                    )?;
                    if extracted_chars > MAX_EXTRACTED_CHARS {
                        return Err(ToolError::FileTooLarge {
                            size: u64::try_from(extracted_chars).unwrap_or(u64::MAX),
                            max: MAX_EXTRACTED_CHARS as u64,
                        }
                        .into());
                    }
                    all_text.push_str(&format!("\n--- Page {} ---\n", page_num));
                    all_text.push_str(&text);
                    all_text.push('\n');
                }
            }
            Err(_) => continue, // Skip pages that fail to extract
        }
    }

    Ok(all_text)
}

/// Extract PDF metadata from a parsed Document.
fn extract_pdf_metadata_from_doc(doc: &lopdf::Document) -> Value {
    let page_count = doc.get_pages().len();
    let mut meta = serde_json::json!({ "pages": page_count });

    // Extract info dictionary fields (title, author, subject, creator, producer, creation date)
    if let Ok(info_obj) = doc.trailer.get(b"Info")
        && let Ok(info_dict) = info_obj.as_dict()
    {
        let fields = [
            ("Title", "title"),
            ("Author", "author"),
            ("Subject", "subject"),
            ("Creator", "creator"),
            ("Producer", "producer"),
            ("CreationDate", "creation_date"),
            ("ModDate", "modification_date"),
        ];
        for (pdf_key, json_key) in &fields {
            if let Ok(text_obj) = info_dict.get(pdf_key.as_bytes())
                && let Ok(text_str) = text_obj.as_str()
                && let Some(meta) = meta.as_object_mut()
            {
                meta.insert(
                    (*json_key).to_string(),
                    serde_json::Value::String(String::from_utf8_lossy(text_str).to_string()),
                );
            }
        }
    }

    meta
}

/// Parse page range specification into a list of page numbers.
fn parse_page_range(spec: &str, total_pages: u32) -> Result<Vec<u32>> {
    if spec == "all" {
        return Ok((1..=total_pages).collect());
    }

    let mut pages = Vec::new();

    for part in spec.split(',') {
        let part = part.trim();
        if part.contains('-') {
            let bounds: Vec<&str> = part.split('-').collect();
            if bounds.len() == 2 {
                let start: u32 =
                    bounds[0]
                        .trim()
                        .parse()
                        .map_err(|_| ToolError::InvalidParameter {
                            name: "pages".to_string(),
                            message: format!("Invalid page number: '{}'", bounds[0]),
                        })?;
                let end: u32 =
                    bounds[1]
                        .trim()
                        .parse()
                        .map_err(|_| ToolError::InvalidParameter {
                            name: "pages".to_string(),
                            message: format!("Invalid page number: '{}'", bounds[1]),
                        })?;
                for p in start..=end.min(total_pages) {
                    pages.push(p);
                }
            }
        } else {
            let p: u32 = part.parse().map_err(|_| ToolError::InvalidParameter {
                name: "pages".to_string(),
                message: format!("Invalid page number: '{}'", part),
            })?;
            if p <= total_pages {
                pages.push(p);
            }
        }
    }

    if pages.is_empty() {
        return Err(ToolError::InvalidParameter {
            name: "pages".to_string(),
            message: "No valid pages specified".to_string(),
        }
        .into());
    }

    pages.sort_unstable();
    pages.dedup();
    Ok(pages)
}
