//! PubMed paper search tool.
//!
//! Queries the NCBI E-utilities API (ESearch + EFetch) and returns structured
//! paper metadata including PMID, title, authors, abstract, journal, DOI, and MeSH terms.

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::Value;
use std::sync::OnceLock;

const TOOL_NAME: &str = "pubmed_search";
const ESEARCH_URL: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi";
const EFETCH_URL: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi";

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

pub struct PubMedSearchTool;

impl Tool for PubMedSearchTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]
    }

    fn description(&self) -> &str {
        "Search PubMed (NCBI) for biomedical literature. Returns PMID, title, authors, abstract, journal, year, DOI, and MeSH terms. Supports date range filtering and sorting. Example: pubmed_search(query='CRISPR gene therapy cancer', max_results=20, sort='pub_date')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (keywords, MeSH terms, author names, etc.)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results (default 20, max 100)"
                },
                "sort": {
                    "type": "string",
                    "description": "Sort order: 'relevance' (default), 'pub_date', 'first_author'"
                },
                "min_date": {
                    "type": "string",
                    "description": "Minimum publication date (optional). Format: YYYY/MM/DD"
                },
                "max_date": {
                    "type": "string",
                    "description": "Maximum publication date (optional). Format: YYYY/MM/DD"
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

            let sort = parameters
                .get("sort")
                .and_then(|v| v.as_str())
                .unwrap_or("relevance");

            let min_date = parameters.get("min_date").and_then(|v| v.as_str());
            let max_date = parameters.get("max_date").and_then(|v| v.as_str());

            // Step 1: ESearch to get PMIDs
            let mut search_url = format!(
                "{}?db=pubmed&term={}&retmax={}&retmode=json&sort={}",
                ESEARCH_URL,
                urlencoding::encode(query),
                max_results,
                match sort {
                    "pub_date" => "pub+date",
                    "first_author" => "first+author",
                    _ => "relevance",
                }
            );

            if let Some(d) = min_date {
                search_url.push_str(&format!(
                    "&mindate={}&datetype=pdat",
                    urlencoding::encode(d)
                ));
            }
            if let Some(d) = max_date {
                search_url.push_str(&format!(
                    "&maxdate={}&datetype=pdat",
                    urlencoding::encode(d)
                ));
            }

            let client = shared_client();

            let response =
                client
                    .get(&search_url)
                    .send()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("PubMed ESearch request failed: {}", e),
                    })?;

            let status = response.status();
            if !status.is_success() {
                let body = crate::http_body::read_bounded_text(
                    response,
                    crate::http_body::MAX_API_RESPONSE_BYTES,
                    TOOL_NAME,
                    None,
                )
                .await?;
                return Err(ToolError::ExecutionFailed {
                    tool: TOOL_NAME.to_string(),
                    message: format!("PubMed ESearch error ({}): {}", status, body),
                }
                .into());
            }

            let json: Value = crate::http_body::read_bounded_json(
                response,
                crate::http_body::MAX_API_RESPONSE_BYTES,
                TOOL_NAME,
                None,
            )
            .await?;

            let id_list = json
                .get("esearchresult")
                .and_then(|v| v.get("idlist"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let total_count = json
                .get("esearchresult")
                .and_then(|v| v.get("count"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);

            if id_list.is_empty() {
                let result = serde_json::json!({
                    "query": query,
                    "total_results": total_count,
                    "papers": []
                });
                return Ok(ToolResult::success_json(result));
            }

            // Step 2: EFetch to get paper details
            let pmids: Vec<String> = id_list
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();

            let fetch_url = format!(
                "{}?db=pubmed&id={}&rettype=xml&retmode=xml",
                EFETCH_URL,
                pmids.join(",")
            );

            let response =
                client
                    .get(&fetch_url)
                    .send()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("PubMed EFetch request failed: {}", e),
                    })?;

            let xml_text = crate::http_body::read_bounded_text(
                response,
                crate::http_body::MAX_API_RESPONSE_BYTES,
                TOOL_NAME,
                None,
            )
            .await?;

            let papers = parse_pubmed_xml(&xml_text)?;

            let result = serde_json::json!({
                "query": query,
                "total_results": total_count,
                "returned": papers.len(),
                "papers": papers,
            });

            Ok(ToolResult::success_json(result))
        })
    }
}

/// Parse PubMed XML response into structured paper records.
fn parse_pubmed_xml(xml: &str) -> Result<Vec<Value>> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut papers = Vec::new();
    let mut in_article = false;
    let mut in_medline_citation = false;
    let mut in_journal = false;
    let mut in_author = false;
    let mut in_mesh_heading = false;
    let mut current_tag = String::new();

    // Current article fields
    let mut pmid = String::new();
    let mut title = String::new();
    let mut abstract_text = String::new();
    let mut journal = String::new();
    let mut year = String::new();
    let mut doi = String::new();
    let mut authors: Vec<String> = Vec::new();
    let mut mesh_terms: Vec<String> = Vec::new();
    let mut author_last = String::new();
    let mut author_first = String::new();
    let mut mesh_descriptor = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag.as_str() {
                    "PubmedArticle" => {
                        in_article = true;
                        pmid.clear();
                        title.clear();
                        abstract_text.clear();
                        journal.clear();
                        year.clear();
                        doi.clear();
                        authors.clear();
                        mesh_terms.clear();
                    }
                    "MedlineCitation" if in_article => {
                        in_medline_citation = true;
                    }
                    "Journal" if in_medline_citation => {
                        in_journal = true;
                    }
                    "Author" if in_medline_citation => {
                        in_author = true;
                        author_last.clear();
                        author_first.clear();
                    }
                    "MeshHeading" if in_medline_citation => {
                        in_mesh_heading = true;
                        mesh_descriptor.clear();
                    }
                    "PMID" if in_medline_citation => current_tag = "pmid".to_string(),
                    "ArticleTitle" if in_medline_citation => current_tag = "title".to_string(),
                    "AbstractText" if in_medline_citation => current_tag = "abstract".to_string(),
                    "Title" if in_journal => current_tag = "journal".to_string(),
                    "Year" if in_journal => current_tag = "year".to_string(),
                    "LastName" if in_author => current_tag = "last_name".to_string(),
                    "ForeName" if in_author => current_tag = "fore_name".to_string(),
                    "DescriptorName" if in_mesh_heading => {
                        current_tag = "mesh_descriptor".to_string()
                    }
                    "ArticleId" if in_article => current_tag = "article_id".to_string(),
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                match current_tag.as_str() {
                    "pmid" if in_medline_citation => pmid = text,
                    "title" if in_medline_citation => title = text,
                    "abstract" if in_medline_citation => abstract_text = text,
                    "journal" if in_journal => journal = text,
                    "year" if in_journal => year = text,
                    "last_name" if in_author => author_last = text,
                    "fore_name" if in_author => author_first = text,
                    "mesh_descriptor" if in_mesh_heading => mesh_descriptor = text,
                    "article_id"
                        if in_article
                        // Check if this is a DOI by looking at IdType attribute
                        // (we already consumed the start event, so check text content)
                        && text.contains("10.") =>
                    {
                        doi = text;
                    }
                    _ => {}
                }
                current_tag.clear();
            }
            Ok(Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "ArticleId" && in_article {
                    // Check for DOI in attributes
                    let mut id_type = String::new();
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"IdType" {
                            id_type = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                    if id_type == "doi" {
                        // The DOI value is in the text content, which we already consumed
                        // For empty elements, the value might be in the attribute
                    }
                }
            }
            Ok(Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag.as_str() {
                    "PubmedArticle" if in_article => {
                        papers.push(serde_json::json!({
                            "pmid": pmid,
                            "title": title.trim(),
                            "authors": authors,
                            "abstract": abstract_text.trim(),
                            "journal": journal.trim(),
                            "year": year,
                            "doi": doi,
                            "mesh_terms": mesh_terms,
                        }));
                        in_article = false;
                    }
                    "MedlineCitation" => {
                        in_medline_citation = false;
                    }
                    "Journal" => {
                        in_journal = false;
                    }
                    "Author" if in_author => {
                        let name = if !author_last.is_empty() && !author_first.is_empty() {
                            format!("{} {}", author_last, author_first)
                        } else if !author_last.is_empty() {
                            author_last.clone()
                        } else {
                            String::new()
                        };
                        if !name.is_empty() {
                            authors.push(name);
                        }
                        in_author = false;
                    }
                    "MeshHeading" => {
                        if in_mesh_heading && !mesh_descriptor.is_empty() {
                            mesh_terms.push(mesh_descriptor.clone());
                        }
                        in_mesh_heading = false;
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
