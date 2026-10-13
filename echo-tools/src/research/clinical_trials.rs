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

            let url = clinical_trials_url(
                query,
                max_results,
                status_filter,
                phase_filter,
                country_filter,
            );

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

            parse_clinical_trials_result(query, &json)
        })
    }
}

// Keep request construction and result normalization in the original Tool's
// boundary so offline response regressions exercise the real production path.
fn clinical_trials_url(
    query: &str,
    max_results: usize,
    status_filter: Option<&str>,
    phase_filter: Option<&str>,
    country_filter: Option<&str>,
) -> String {
    // Build query string
    let mut url = format!(
        "{}?query.term={}&pageSize={}&format=json&countTotal=true",
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
    url
}

fn parse_clinical_trials_result(query: &str, json: &Value) -> Result<ToolResult> {
    // API v2 omits totalCount unless requested. Absence is not a zero match
    // count, and malformed counts must not become successful default values.
    let total_count = json
        .get("totalCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| ToolError::ExecutionFailed {
            tool: TOOL_NAME.to_string(),
            message: "ClinicalTrials.gov API response requires a non-negative integer totalCount after countTotal=true"
                .to_string(),
        })?;

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
                            let name = iv.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let desc = iv.get("description").and_then(|v| v.as_str()).unwrap_or("");
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
                                let time =
                                    o.get("timeFrame").and_then(|v| v.as_str()).unwrap_or("");
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::error::ReactError;

    fn official_response(name: &str) -> Result<Value> {
        let payload = match name {
            "default" => {
                include_str!("../../../docs/evidence/clinical-trials-total-fidelity/default.json")
            }
            "requested" => {
                include_str!("../../../docs/evidence/clinical-trials-total-fidelity/requested.json")
            }
            _ => {
                return Err(ReactError::Other(
                    "unknown clinical trials fixture".to_string(),
                ));
            }
        };
        serde_json::from_str(payload).map_err(|error| ReactError::Other(error.to_string()))
    }

    #[test]
    fn clinical_trials_requests_server_total_count() -> Result<()> {
        let url = reqwest::Url::parse(&clinical_trials_url("中文 CRISPR", 1, None, None, None))
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("countTotal").map(String::as_str), Some("true"));
        assert_eq!(
            query.get("query.term").map(String::as_str),
            Some("中文 CRISPR")
        );
        assert_eq!(query.get("pageSize").map(String::as_str), Some("1"));
        assert_eq!(query.get("format").map(String::as_str), Some("json"));
        Ok(())
    }

    #[test]
    fn clinical_trials_missing_total_is_an_explicit_error() -> Result<()> {
        let json = official_response("default")?;
        assert!(json.get("totalCount").is_none());
        assert_eq!(
            json.get("studies").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );
        assert!(parse_clinical_trials_result("agent artificial intelligence", &json).is_err());
        Ok(())
    }

    #[test]
    fn clinical_trials_malformed_total_is_an_explicit_error() {
        for total in [
            Value::Null,
            serde_json::json!("386"),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!(false),
            serde_json::json!({}),
            serde_json::json!([]),
        ] {
            let response = serde_json::json!({"totalCount": total, "studies": []});
            let error = parse_clinical_trials_result("query", &response).err();
            assert!(
                matches!(error, Some(ReactError::Tool(inner)) if matches!(inner.as_ref(), ToolError::ExecutionFailed { .. }))
            );
        }
    }

    #[test]
    fn clinical_trials_preserves_authoritative_totals_and_page_size() -> Result<()> {
        let response = official_response("requested")?;
        let result = parse_clinical_trials_result("agent artificial intelligence", &response)?;
        let data = result.data.ok_or_else(|| {
            ReactError::Other("expected structured clinical trial result".to_string())
        })?;
        assert_eq!(data.get("total_results"), Some(&serde_json::json!(386)));
        assert_eq!(data.get("returned"), Some(&serde_json::json!(1)));
        assert_eq!(
            data.get("studies").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );
        for total in [0_u64, u64::MAX] {
            let result = parse_clinical_trials_result(
                "query",
                &serde_json::json!({"totalCount":total,"studies":[]}),
            )?;
            assert_eq!(
                result
                    .data
                    .as_ref()
                    .and_then(|data| data.get("total_results")),
                Some(&serde_json::json!(total))
            );
            assert_eq!(
                result.data.as_ref().and_then(|data| data.get("returned")),
                Some(&serde_json::json!(0))
            );
        }
        Ok(())
    }
}
