//! ClinicalTrials.gov search tool.
//!
//! Queries the ClinicalTrials.gov API v2 and returns structured clinical trial
//! metadata including NCT ID, title, status, phase, conditions, interventions,
//! and primary outcome measures.

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::Value;
use std::sync::OnceLock;

const TOOL_NAME: &str = "clinical_trials_search";
const CT_API_URL: &str = "https://clinicaltrials.gov/api/v2/studies";

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

pub struct ClinicalTrialsSearchTool;

impl Tool for ClinicalTrialsSearchTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]
    }

    fn description(&self) -> &str {
        "Search ClinicalTrials.gov for clinical trials. Returns NCT ID, title, status, phase, conditions, interventions, and primary outcome measures. No API key required. Example: clinical_trials_search(query='CRISPR cancer immunotherapy', max_results=10, status='RECRUITING')"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (condition, intervention, NCT ID, etc.)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results (default 10, max 100)"
                },
                "status": {
                    "type": "string",
                    "description": "Filter by study status (optional): RECRUITING, COMPLETED, ACTIVE_NOT_RECRUITING, NOT_YET_RECRUITING, TERMINATED, SUSPENDED, WITHDRAWN"
                },
                "phase": {
                    "type": "string",
                    "description": "Filter by phase (optional): EARLY_PHASE1, PHASE1, PHASE2, PHASE3, PHASE4, NA"
                },
                "country": {
                    "type": "string",
                    "description": "Filter by country (optional). E.g. 'United States', 'China'"
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
                .unwrap_or(10)
                .min(100) as usize;

            let status_filter = parameters.get("status").and_then(|v| v.as_str());
            let phase_filter = parameters.get("phase").and_then(|v| v.as_str());
            let country_filter = parameters.get("country").and_then(|v| v.as_str());

            // Build query string
            let mut url = format!(
                "{}?query.term={}&pageSize={}&format=json",
                CT_API_URL,
                urlencoding::encode(query),
                max_results
            );

            // Add filter parameters
            let mut filter_parts = Vec::new();
            if let Some(s) = status_filter {
                filter_parts.push(format!("overallStatus={}", s));
            }
            if let Some(p) = phase_filter {
                filter_parts.push(format!("phase={}", p));
            }
            if let Some(c) = country_filter {
                filter_parts.push(format!("locn.country={}", urlencoding::encode(c)));
            }
            if !filter_parts.is_empty() {
                url.push_str(&format!("&filter.overall={}", filter_parts.join(",")));
            }

            let client = shared_client();

            let response =
                client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: TOOL_NAME.to_string(),
                        message: format!("ClinicalTrials.gov API request failed: {}", e),
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
                    message: format!("ClinicalTrials.gov API error ({}): {}", status, body),
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

            let total_count = json.get("totalCount").and_then(|v| v.as_u64()).unwrap_or(0);

            let studies = json
                .get("studies")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            // Normalize study format
            let normalized: Vec<Value> = studies
                .iter()
                .map(|study| {
                    let protocol = study.get("protocolSection").cloned().unwrap_or(Value::Null);

                    let nct_id = protocol
                        .get("identificationModule")
                        .and_then(|v| v.get("nctId"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let brief_title = protocol
                        .get("identificationModule")
                        .and_then(|v| v.get("briefTitle"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let official_title = protocol
                        .get("identificationModule")
                        .and_then(|v| v.get("officialTitle"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let overall_status = protocol
                        .get("statusModule")
                        .and_then(|v| v.get("overallStatus"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let phase = protocol
                        .get("designModule")
                        .and_then(|v| v.get("phases"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|p| p.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();

                    let conditions = protocol
                        .get("conditionsModule")
                        .and_then(|v| v.get("conditions"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| c.as_str().map(|s| s.to_string()))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();

                    let interventions = protocol
                        .get("armsInterventionsModule")
                        .and_then(|v| v.get("interventions"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .map(|iv| {
                                    let name = iv
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    let desc = iv
                                        .get("description")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    format!("{}: {}", name, desc)
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();

                    let primary_outcomes = protocol
                        .get("outcomesModule")
                        .and_then(|v| v.get("primaryOutcomes"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|o| {
                                    o.get("measure").and_then(|v| v.as_str()).map(|s| {
                                        let time = o
                                            .get("timeFrame")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        if time.is_empty() {
                                            s.to_string()
                                        } else {
                                            format!("{} [{}]", s, time)
                                        }
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();

                    let enrollment = protocol
                        .get("designModule")
                        .and_then(|v| v.get("enrollmentInfo"))
                        .and_then(|v| v.get("count"))
                        .and_then(|v| v.as_u64());

                    let start_date = protocol
                        .get("statusModule")
                        .and_then(|v| v.get("startDateStruct"))
                        .and_then(|v| v.get("date"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let completion_date = protocol
                        .get("statusModule")
                        .and_then(|v| v.get("primaryCompletionDateStruct"))
                        .and_then(|v| v.get("date"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    serde_json::json!({
                        "nct_id": nct_id,
                        "title": if !official_title.is_empty() { official_title } else { brief_title },
                        "status": overall_status,
                        "phase": phase,
                        "conditions": conditions,
                        "interventions": interventions,
                        "primary_outcomes": primary_outcomes,
                        "enrollment": enrollment,
                        "start_date": start_date,
                        "completion_date": completion_date,
                        "url": format!("https://clinicaltrials.gov/study/{}", nct_id),
                    })
                })
                .collect();

            let result = serde_json::json!({
                "query": query,
                "total_results": total_count,
                "returned": normalized.len(),
                "studies": normalized,
            });

            Ok(ToolResult::success_json(result))
        })
    }
}
