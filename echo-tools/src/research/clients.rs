//! Reusable clients for open scholarly metadata and reference-manager APIs.
//!
//! The clients return a small normalized work contract so applications can
//! persist records using their own storage model. Provider-specific response
//! fields remain available through the dedicated Zotero and Europe PMC types.

use std::collections::BTreeMap;
use std::time::Duration;

use echo_core::error::{ReactError, Result, ToolError};
use reqwest::{Client, Method, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESULTS: usize = 100;
const MAX_ZOTERO_ITEMS: usize = 10_000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScholarlyWork {
    pub provider: String,
    pub provider_id: String,
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    pub abstract_text: Option<String>,
    pub doi: Option<String>,
    pub pmid: Option<String>,
    pub pmcid: Option<String>,
    pub arxiv_id: Option<String>,
    pub openalex_id: Option<String>,
    pub year: Option<i32>,
    pub venue: Option<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScholarlySearchPage {
    pub total: Option<u64>,
    #[serde(default)]
    pub works: Vec<ScholarlyWork>,
}

#[derive(Debug, Clone)]
pub struct OpenAlexClient {
    client: Client,
    base_url: String,
    mailto: Option<String>,
}

impl OpenAlexClient {
    pub fn new(mailto: Option<String>) -> Result<Self> {
        Self::with_base_url("https://api.openalex.org", mailto)
    }

    pub fn with_base_url(base_url: impl Into<String>, mailto: Option<String>) -> Result<Self> {
        Ok(Self {
            client: build_client()?,
            base_url: clean_base_url(base_url),
            mailto: clean_optional(mailto),
        })
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<ScholarlySearchPage> {
        let query = required_query(query)?;
        let mut request = self.client.get(format!("{}/works", self.base_url)).query(&[
            ("search", query),
            ("per-page", &limit.clamp(1, MAX_RESULTS).to_string()),
        ]);
        if let Some(mailto) = self.mailto.as_deref() {
            request = request.query(&[("mailto", mailto)]);
        }
        let value = json_response(
            request
                .send()
                .await
                .map_err(|error| http_error("OpenAlex search", error))?,
            "OpenAlex search",
        )
        .await?;
        parse_openalex_page(&value)
    }

    pub async fn resolve_doi(&self, doi: &str) -> Result<Option<ScholarlyWork>> {
        let doi = normalize_doi(doi).ok_or_else(|| invalid("DOI cannot be empty"))?;
        let mut request = self.client.get(format!("{}/works", self.base_url)).query(&[
            ("filter", format!("doi:{doi}")),
            ("per-page", "1".to_string()),
        ]);
        if let Some(mailto) = self.mailto.as_deref() {
            request = request.query(&[("mailto", mailto)]);
        }
        let value = json_response(
            request
                .send()
                .await
                .map_err(|error| http_error("OpenAlex DOI lookup", error))?,
            "OpenAlex DOI lookup",
        )
        .await?;
        Ok(parse_openalex_page(&value)?.works.into_iter().next())
    }
}

#[derive(Debug, Clone)]
pub struct CrossrefClient {
    client: Client,
    base_url: String,
    mailto: Option<String>,
}

impl CrossrefClient {
    pub fn new(mailto: Option<String>) -> Result<Self> {
        Self::with_base_url("https://api.crossref.org", mailto)
    }

    pub fn with_base_url(base_url: impl Into<String>, mailto: Option<String>) -> Result<Self> {
        Ok(Self {
            client: build_client()?,
            base_url: clean_base_url(base_url),
            mailto: clean_optional(mailto),
        })
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<ScholarlySearchPage> {
        let query = required_query(query)?;
        let mut request = self.client.get(format!("{}/works", self.base_url)).query(&[
            ("query", query),
            ("rows", &limit.clamp(1, MAX_RESULTS).to_string()),
        ]);
        if let Some(mailto) = self.mailto.as_deref() {
            request = request.query(&[("mailto", mailto)]);
        }
        let value = json_response(
            request
                .send()
                .await
                .map_err(|error| http_error("Crossref search", error))?,
            "Crossref search",
        )
        .await?;
        parse_crossref_page(&value)
    }

    pub async fn resolve_doi(&self, doi: &str) -> Result<Option<ScholarlyWork>> {
        let doi = normalize_doi(doi).ok_or_else(|| invalid("DOI cannot be empty"))?;
        let url = format!("{}/works/{}", self.base_url, urlencoding::encode(&doi));
        let mut request = self.client.get(url);
        if let Some(mailto) = self.mailto.as_deref() {
            request = request.query(&[("mailto", mailto)]);
        }
        let response = request
            .send()
            .await
            .map_err(|error| http_error("Crossref DOI lookup", error))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let value = json_response(response, "Crossref DOI lookup").await?;
        value.get("message").map(parse_crossref_work).transpose()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EuropePmcLink {
    pub id: String,
    pub source: Option<String>,
    pub title: Option<String>,
    pub authors: Option<String>,
    pub year: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EuropePmcTerm {
    pub name: String,
    pub semantic_type: Option<String>,
    pub frequency: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct EuropePmcClient {
    client: Client,
    base_url: String,
}

impl EuropePmcClient {
    pub fn new() -> Result<Self> {
        Self::with_base_url("https://www.ebi.ac.uk/europepmc/webservices/rest")
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Result<Self> {
        Ok(Self {
            client: build_client()?,
            base_url: clean_base_url(base_url),
        })
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<ScholarlySearchPage> {
        let query = required_query(query)?;
        let response = self
            .client
            .get(format!("{}/search", self.base_url))
            .query(&[
                ("query", query),
                ("format", "json"),
                ("pageSize", &limit.clamp(1, MAX_RESULTS).to_string()),
            ])
            .send()
            .await
            .map_err(|error| http_error("Europe PMC search", error))?;
        let value = json_response(response, "Europe PMC search").await?;
        parse_europe_pmc_page(&value)
    }

    pub async fn citations(&self, source: &str, id: &str) -> Result<Vec<EuropePmcLink>> {
        self.links(source, id, "citations").await
    }

    pub async fn references(&self, source: &str, id: &str) -> Result<Vec<EuropePmcLink>> {
        self.links(source, id, "references").await
    }

    pub async fn text_mined_terms(&self, source: &str, id: &str) -> Result<Vec<EuropePmcTerm>> {
        let response = self
            .client
            .get(format!(
                "{}/{}/{}/textMinedTerms",
                self.base_url,
                encode_path(source)?,
                encode_path(id)?
            ))
            .query(&[("format", "json")])
            .send()
            .await
            .map_err(|error| http_error("Europe PMC text-mined terms", error))?;
        let value = json_response(response, "Europe PMC text-mined terms").await?;
        Ok(parse_europe_pmc_terms(&value))
    }

    pub async fn full_text_xml(&self, pmcid: &str) -> Result<String> {
        let response = self
            .client
            .get(format!(
                "{}/{}/fullTextXML",
                self.base_url,
                encode_path(pmcid)?
            ))
            .send()
            .await
            .map_err(|error| http_error("Europe PMC full text", error))?;
        text_response(response, "Europe PMC full text").await
    }

    async fn links(&self, source: &str, id: &str, relation: &str) -> Result<Vec<EuropePmcLink>> {
        let response = self
            .client
            .get(format!(
                "{}/{}/{}/{}",
                self.base_url,
                encode_path(source)?,
                encode_path(id)?,
                relation
            ))
            .query(&[("format", "json"), ("pageSize", "100")])
            .send()
            .await
            .map_err(|error| http_error("Europe PMC links", error))?;
        let value = json_response(response, "Europe PMC links").await?;
        Ok(parse_europe_pmc_links(&value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoteroLibraryKind {
    User,
    Group,
}

impl ZoteroLibraryKind {
    fn path_segment(self) -> &'static str {
        match self {
            Self::User => "users",
            Self::Group => "groups",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoteroCreator {
    pub creator_type: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoteroTag {
    pub tag: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoteroItemData {
    pub item_type: String,
    pub title: String,
    #[serde(default)]
    pub creators: Vec<ZoteroCreator>,
    pub abstract_note: Option<String>,
    pub publication_title: Option<String>,
    pub date: Option<String>,
    #[serde(rename = "DOI")]
    pub doi: Option<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub tags: Vec<ZoteroTag>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ZoteroItem {
    pub key: String,
    pub version: u64,
    pub data: ZoteroItemData,
}

#[derive(Debug, Clone)]
pub struct ZoteroClient {
    client: Client,
    base_url: String,
    library_kind: ZoteroLibraryKind,
    library_id: String,
    api_key: String,
}

impl ZoteroClient {
    pub fn new(
        library_kind: ZoteroLibraryKind,
        library_id: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self> {
        Self::with_base_url("https://api.zotero.org", library_kind, library_id, api_key)
    }

    pub fn with_base_url(
        base_url: impl Into<String>,
        library_kind: ZoteroLibraryKind,
        library_id: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self> {
        let library_id = library_id.into().trim().to_string();
        let api_key = api_key.into().trim().to_string();
        if library_id.is_empty() || api_key.is_empty() {
            return Err(invalid("Zotero library ID and API key are required"));
        }
        Ok(Self {
            client: build_client()?,
            base_url: clean_base_url(base_url),
            library_kind,
            library_id,
            api_key,
        })
    }

    pub async fn list_items(&self, limit: usize) -> Result<Vec<ZoteroItem>> {
        let target = limit.clamp(1, MAX_ZOTERO_ITEMS);
        let mut items = Vec::new();
        let mut start = 0usize;
        loop {
            let page_size = target.saturating_sub(items.len()).min(MAX_RESULTS);
            if page_size == 0 {
                break;
            }
            let response = self
                .request(Method::GET, format!("{}/items", self.library_path()))
                .query(&[
                    ("format", "json"),
                    ("limit", &page_size.to_string()),
                    ("start", &start.to_string()),
                ])
                .send()
                .await
                .map_err(|error| http_error("Zotero item list", error))?;
            let total = response
                .headers()
                .get("Total-Results")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<usize>().ok());
            let value = json_response(response, "Zotero item list").await?;
            let mut page: Vec<ZoteroItem> = serde_json::from_value(value)
                .map_err(|error| json_error("Zotero item list", error))?;
            let returned = page.len();
            items.append(&mut page);
            start = start.saturating_add(returned);
            if returned == 0 || items.len() >= target || total.is_some_and(|total| start >= total) {
                break;
            }
        }
        Ok(items)
    }

    pub async fn create_items(&self, items: &[ZoteroItemData]) -> Result<Value> {
        if items.is_empty() {
            return Err(invalid("at least one Zotero item is required"));
        }
        let response = self
            .request(Method::POST, format!("{}/items", self.library_path()))
            .json(items)
            .send()
            .await
            .map_err(|error| http_error("Zotero item creation", error))?;
        json_response(response, "Zotero item creation").await
    }

    fn library_path(&self) -> String {
        format!(
            "{}/{}/{library_id}",
            self.base_url,
            self.library_kind.path_segment(),
            library_id = self.library_id
        )
    }

    fn request(&self, method: Method, url: String) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .header("Zotero-API-Version", "3")
            .header("Zotero-API-Key", &self.api_key)
    }
}

pub fn scholarly_work_from_zotero(item: &ZoteroItem) -> ScholarlyWork {
    ScholarlyWork {
        provider: "zotero".to_string(),
        provider_id: item.key.clone(),
        title: item.data.title.clone(),
        authors: item
            .data
            .creators
            .iter()
            .filter_map(zotero_creator_name)
            .collect(),
        abstract_text: clean_optional(item.data.abstract_note.clone()),
        doi: item.data.doi.as_deref().and_then(normalize_doi),
        year: item.data.date.as_deref().and_then(first_year),
        venue: clean_optional(item.data.publication_title.clone()),
        url: clean_optional(item.data.url.clone()),
        keywords: item
            .data
            .tags
            .iter()
            .filter_map(|tag| clean_optional(Some(tag.tag.clone())))
            .collect(),
        ..ScholarlyWork::default()
    }
}

pub fn scholarly_work_to_zotero(work: &ScholarlyWork) -> ZoteroItemData {
    ZoteroItemData {
        item_type: "journalArticle".to_string(),
        title: work.title.clone(),
        creators: work
            .authors
            .iter()
            .filter_map(|name| {
                clean_optional(Some(name.clone())).map(|name| ZoteroCreator {
                    creator_type: "author".to_string(),
                    name: Some(name),
                    ..ZoteroCreator::default()
                })
            })
            .collect(),
        abstract_note: work.abstract_text.clone(),
        publication_title: work.venue.clone(),
        date: work.year.map(|year| year.to_string()),
        doi: work.doi.clone(),
        url: work.url.clone(),
        tags: work
            .keywords
            .iter()
            .map(|tag| ZoteroTag { tag: tag.clone() })
            .collect(),
        extra: BTreeMap::new(),
    }
}

fn parse_openalex_page(value: &Value) -> Result<ScholarlySearchPage> {
    let works = value
        .get("results")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_openalex_work).collect())
        .transpose()?
        .unwrap_or_default();
    Ok(ScholarlySearchPage {
        total: value
            .get("meta")
            .and_then(|meta| meta.get("count"))
            .and_then(Value::as_u64),
        works,
    })
}

fn parse_openalex_work(value: &Value) -> Result<ScholarlyWork> {
    let provider_id = string_at(value, &["id"]).unwrap_or_default();
    let openalex_id = strip_url_identifier(&provider_id, "https://openalex.org/");
    let ids = value.get("ids").unwrap_or(&Value::Null);
    let primary_location = value.get("primary_location").unwrap_or(&Value::Null);
    Ok(ScholarlyWork {
        provider: "openalex".to_string(),
        provider_id,
        title: string_at(value, &["title"]).unwrap_or_default(),
        authors: value
            .get("authorships")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| string_at(item, &["author", "display_name"]))
                    .collect()
            })
            .unwrap_or_default(),
        abstract_text: inverted_abstract(value.get("abstract_inverted_index")),
        doi: string_at(value, &["doi"]).and_then(|doi| normalize_doi(&doi)),
        pmid: string_at(ids, &["pmid"])
            .and_then(|id| strip_url_identifier(&id, "https://pubmed.ncbi.nlm.nih.gov/")),
        pmcid: string_at(ids, &["pmcid"])
            .and_then(|id| strip_url_identifier(&id, "https://www.ncbi.nlm.nih.gov/pmc/articles/")),
        openalex_id,
        year: value
            .get("publication_year")
            .and_then(Value::as_i64)
            .and_then(|year| i32::try_from(year).ok()),
        venue: string_at(primary_location, &["source", "display_name"]),
        url: string_at(primary_location, &["landing_page_url"])
            .or_else(|| string_at(value, &["doi"])),
        keywords: value
            .get("keywords")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| string_at(item, &["display_name"]))
                    .collect()
            })
            .unwrap_or_default(),
        ..ScholarlyWork::default()
    })
}

fn parse_crossref_page(value: &Value) -> Result<ScholarlySearchPage> {
    let message = value.get("message").unwrap_or(&Value::Null);
    let works = message
        .get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_crossref_work).collect())
        .transpose()?
        .unwrap_or_default();
    Ok(ScholarlySearchPage {
        total: message.get("total-results").and_then(Value::as_u64),
        works,
    })
}

fn parse_crossref_work(value: &Value) -> Result<ScholarlyWork> {
    let doi = string_at(value, &["DOI"]).and_then(|value| normalize_doi(&value));
    let provider_id = doi
        .clone()
        .or_else(|| string_at(value, &["URL"]))
        .unwrap_or_default();
    Ok(ScholarlyWork {
        provider: "crossref".to_string(),
        provider_id,
        title: first_string(value.get("title")).unwrap_or_default(),
        authors: value
            .get("author")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(crossref_author_name).collect())
            .unwrap_or_default(),
        abstract_text: string_at(value, &["abstract"]),
        doi,
        year: crossref_year(value),
        venue: first_string(value.get("container-title")),
        url: string_at(value, &["URL"]),
        keywords: value
            .get("subject")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        ..ScholarlyWork::default()
    })
}

fn parse_europe_pmc_page(value: &Value) -> Result<ScholarlySearchPage> {
    let works = value
        .get("resultList")
        .and_then(|list| list.get("result"))
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_europe_pmc_work).collect())
        .transpose()?
        .unwrap_or_default();
    Ok(ScholarlySearchPage {
        total: value.get("hitCount").and_then(Value::as_u64),
        works,
    })
}

fn parse_europe_pmc_work(value: &Value) -> Result<ScholarlyWork> {
    let source = string_at(value, &["source"]).unwrap_or_default();
    let id = string_at(value, &["id"]).unwrap_or_default();
    Ok(ScholarlyWork {
        provider: "europe_pmc".to_string(),
        provider_id: if source.is_empty() {
            id.clone()
        } else {
            format!("{source}:{id}")
        },
        title: string_at(value, &["title"]).unwrap_or_default(),
        authors: string_at(value, &["authorString"])
            .map(|authors| {
                authors
                    .split(',')
                    .filter_map(|name| clean_optional(Some(name.to_string())))
                    .collect()
            })
            .unwrap_or_default(),
        abstract_text: string_at(value, &["abstractText"]),
        doi: string_at(value, &["doi"]).and_then(|doi| normalize_doi(&doi)),
        pmid: (source.eq_ignore_ascii_case("MED") || id.chars().all(|ch| ch.is_ascii_digit()))
            .then_some(id.clone()),
        pmcid: string_at(value, &["pmcid"]),
        year: string_at(value, &["pubYear"]).and_then(|year| year.parse::<i32>().ok()),
        venue: string_at(value, &["journalTitle"]),
        url: (!id.is_empty()).then(|| format!("https://europepmc.org/article/{source}/{id}")),
        ..ScholarlyWork::default()
    })
}

fn parse_europe_pmc_links(value: &Value) -> Vec<EuropePmcLink> {
    ["citationList", "referenceList"]
        .iter()
        .find_map(|key| {
            value
                .get(key)
                .and_then(|list| list.get("citation").or_else(|| list.get("reference")))
                .and_then(Value::as_array)
        })
        .map(|items| {
            items
                .iter()
                .map(|item| EuropePmcLink {
                    id: string_at(item, &["id"])
                        .or_else(|| string_at(item, &["citedById"]))
                        .unwrap_or_default(),
                    source: string_at(item, &["source"]),
                    title: string_at(item, &["title"]),
                    authors: string_at(item, &["authorString"]),
                    year: string_at(item, &["pubYear"]).and_then(|year| year.parse::<i32>().ok()),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_europe_pmc_terms(value: &Value) -> Vec<EuropePmcTerm> {
    value
        .get("semanticTypeList")
        .and_then(|list| list.get("semanticType"))
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .flat_map(|group| {
                    let semantic_type = string_at(group, &["name"]);
                    group
                        .get("tmSummaryList")
                        .and_then(|list| list.get("tmSummary"))
                        .and_then(Value::as_array)
                        .map(|terms| {
                            terms
                                .iter()
                                .filter_map(|term| {
                                    string_at(term, &["term"]).map(|name| EuropePmcTerm {
                                        name,
                                        semantic_type: semantic_type.clone(),
                                        frequency: term.get("count").and_then(Value::as_u64),
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn build_client() -> Result<Client> {
    Client::builder()
        .timeout(DEFAULT_TIMEOUT)
        .user_agent("echo-agent/0.2 scholarly-research")
        .redirect(crate::security::ssrf_safe_redirect_policy())
        .build()
        .map_err(|error| http_error("research client setup", error))
}

async fn json_response(response: Response, operation: &str) -> Result<Value> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| http_error(operation, error))?;
    if !status.is_success() {
        return Err(invalid(format!(
            "{operation} failed with HTTP {status}: {}",
            body.chars().take(500).collect::<String>()
        )));
    }
    serde_json::from_str(&body).map_err(|error| json_error(operation, error))
}

async fn text_response(response: Response, operation: &str) -> Result<String> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| http_error(operation, error))?;
    if !status.is_success() {
        return Err(invalid(format!(
            "{operation} failed with HTTP {status}: {}",
            body.chars().take(500).collect::<String>()
        )));
    }
    Ok(body)
}

fn invalid(message: impl Into<String>) -> ReactError {
    ToolError::InvalidParameter {
        name: "research_api".to_string(),
        message: message.into(),
    }
    .into()
}

fn http_error(operation: &str, error: reqwest::Error) -> ReactError {
    ToolError::ExecutionFailed {
        tool: "research_api".to_string(),
        message: format!("{operation} request failed: {error}"),
    }
    .into()
}

fn json_error(operation: &str, error: serde_json::Error) -> ReactError {
    ToolError::ExecutionFailed {
        tool: "research_api".to_string(),
        message: format!("{operation} returned invalid JSON: {error}"),
    }
    .into()
}

fn required_query(query: &str) -> Result<&str> {
    let query = query.trim();
    if query.is_empty() {
        return Err(invalid("query cannot be empty"));
    }
    Ok(query)
}

fn clean_base_url(value: impl Into<String>) -> String {
    value.into().trim().trim_end_matches('/').to_string()
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

fn normalize_doi(value: &str) -> Option<String> {
    let mut doi = value.trim().to_lowercase();
    for prefix in ["https://doi.org/", "http://doi.org/", "doi:"] {
        if doi.starts_with(prefix) {
            doi = doi.chars().skip(prefix.chars().count()).collect();
            break;
        }
    }
    clean_optional(Some(doi))
}

fn strip_url_identifier(value: &str, prefix: &str) -> Option<String> {
    let stripped = value
        .strip_prefix(prefix)
        .unwrap_or(value)
        .trim_matches('/')
        .to_string();
    clean_optional(Some(stripped))
}

fn encode_path(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid("path identifier cannot be empty"));
    }
    Ok(urlencoding::encode(value).to_string())
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .and_then(|text| clean_optional(Some(text.to_string())))
}

fn first_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .and_then(|text| clean_optional(Some(text.to_string())))
}

fn crossref_author_name(value: &Value) -> Option<String> {
    let given = string_at(value, &["given"]).unwrap_or_default();
    let family = string_at(value, &["family"]).unwrap_or_default();
    clean_optional(Some(format!("{given} {family}")))
}

fn crossref_year(value: &Value) -> Option<i32> {
    ["published-print", "published-online", "issued"]
        .iter()
        .find_map(|key| {
            value
                .get(key)
                .and_then(|date| date.get("date-parts"))
                .and_then(Value::as_array)
                .and_then(|parts| parts.first())
                .and_then(Value::as_array)
                .and_then(|part| part.first())
                .and_then(Value::as_i64)
                .and_then(|year| i32::try_from(year).ok())
        })
}

fn first_year(value: &str) -> Option<i32> {
    let digits: String = value.chars().filter(char::is_ascii_digit).take(4).collect();
    (digits.chars().count() == 4)
        .then(|| digits.parse::<i32>().ok())
        .flatten()
}

fn zotero_creator_name(value: &ZoteroCreator) -> Option<String> {
    value
        .name
        .clone()
        .and_then(|name| clean_optional(Some(name)))
        .or_else(|| {
            clean_optional(Some(format!(
                "{} {}",
                value.first_name.as_deref().unwrap_or(""),
                value.last_name.as_deref().unwrap_or("")
            )))
        })
}

fn inverted_abstract(value: Option<&Value>) -> Option<String> {
    let index = value?.as_object()?;
    let mut words = Vec::new();
    for (word, positions) in index {
        for position in positions
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
        {
            words.push((position, word.clone()));
        }
    }
    words.sort_by_key(|(position, _)| *position);
    clean_optional(Some(
        words
            .into_iter()
            .map(|(_, word)| word)
            .collect::<Vec<_>>()
            .join(" "),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_provider_contracts() -> Result<()> {
        let openalex = parse_openalex_page(&json!({
            "meta": {"count": 1},
            "results": [{
                "id": "https://openalex.org/W1",
                "title": "A study",
                "doi": "https://doi.org/10.1000/test",
                "publication_year": 2025,
                "authorships": [{"author": {"display_name": "Ada Lovelace"}}],
                "abstract_inverted_index": {"hello": [0], "world": [1]},
                "primary_location": {"source": {"display_name": "Journal"}, "landing_page_url": "https://example.test"}
            }]
        }))?;
        assert_eq!(
            openalex
                .works
                .first()
                .and_then(|work| work.openalex_id.as_deref()),
            Some("W1")
        );
        assert_eq!(
            openalex
                .works
                .first()
                .and_then(|work| work.abstract_text.as_deref()),
            Some("hello world")
        );

        let crossref = parse_crossref_page(&json!({
            "message": {"total-results": 1, "items": [{
                "DOI": "10.1000/test", "title": ["A study"],
                "author": [{"given": "Ada", "family": "Lovelace"}],
                "published-online": {"date-parts": [[2025, 1, 1]]}
            }]}
        }))?;
        assert_eq!(
            crossref.works.first().and_then(|work| work.year),
            Some(2025)
        );

        let europe = parse_europe_pmc_page(&json!({
            "hitCount": 1,
            "resultList": {"result": [{"source": "MED", "id": "123", "title": "Trial", "pubYear": "2024"}]}
        }))?;
        assert_eq!(
            europe.works.first().and_then(|work| work.pmid.as_deref()),
            Some("123")
        );
        Ok(())
    }

    #[test]
    fn zotero_conversion_preserves_citation_fields() -> Result<()> {
        let item: ZoteroItem = serde_json::from_value(json!({
            "key": "ABC", "version": 1,
            "data": {
                "itemType": "journalArticle", "title": "Paper", "DOI": "10.1/test",
                "date": "2025-02-03", "creators": [{"creatorType": "author", "name": "A. Author"}],
                "tags": [{"tag": "evidence"}]
            }
        }))?;
        let work = scholarly_work_from_zotero(&item);
        assert_eq!(work.provider_id, "ABC");
        assert_eq!(work.year, Some(2025));
        assert_eq!(
            scholarly_work_to_zotero(&work).doi.as_deref(),
            Some("10.1/test")
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "opt-in live smoke test; set ECHO_AGENT_PROVIDER_SMOKE=1"]
    async fn live_open_scholarly_providers_return_results() -> Result<()> {
        if std::env::var("ECHO_AGENT_PROVIDER_SMOKE").as_deref() != Ok("1") {
            return Err(invalid(
                "set ECHO_AGENT_PROVIDER_SMOKE=1 before running ignored provider smoke tests",
            ));
        }
        let mailto = std::env::var("OPENALEX_MAILTO").ok();
        let openalex = OpenAlexClient::new(mailto.clone())?
            .search("systematic review", 1)
            .await?;
        let crossref = CrossrefClient::new(mailto)?
            .search("systematic review", 1)
            .await?;
        let europe_pmc = EuropePmcClient::new()?
            .search("systematic review", 1)
            .await?;
        if openalex.works.is_empty() || crossref.works.is_empty() || europe_pmc.works.is_empty() {
            return Err(invalid(
                "one or more scholarly providers returned an empty smoke-test page",
            ));
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires ZOTERO_API_KEY and ZOTERO_LIBRARY_ID"]
    async fn live_zotero_credentials_can_list_items() -> Result<()> {
        let api_key = std::env::var("ZOTERO_API_KEY")
            .map_err(|_| invalid("ZOTERO_API_KEY is required for the Zotero smoke test"))?;
        let library_id = std::env::var("ZOTERO_LIBRARY_ID")
            .map_err(|_| invalid("ZOTERO_LIBRARY_ID is required for the Zotero smoke test"))?;
        let library_kind = match std::env::var("ZOTERO_LIBRARY_KIND")
            .unwrap_or_else(|_| "user".to_string())
            .as_str()
        {
            "user" => ZoteroLibraryKind::User,
            "group" => ZoteroLibraryKind::Group,
            value => {
                return Err(invalid(format!(
                    "ZOTERO_LIBRARY_KIND must be user or group, got {value}"
                )));
            }
        };
        ZoteroClient::new(library_kind, library_id, api_key)?
            .list_items(1)
            .await?;
        Ok(())
    }
}
