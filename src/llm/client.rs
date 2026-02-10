use crate::error::{LlmError, Result};
use crate::llm::ChatCompletionRequest;
use crate::llm::types::ChatCompletionResponse;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use tracing::debug;

pub async fn post(
    client: Arc<Client>,
    request_body: &ChatCompletionRequest,
    header_map: HeaderMap,
    url: &str,
) -> Result<ChatCompletionResponse> {
    debug!(
        "Post completion request_body: {}",
        serde_json::to_string(request_body).unwrap_or_else(|e| format!("<serialize error: {}>", e))
    );
    let response = client
        .post(url)
        .headers(header_map)
        .json(request_body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(LlmError::ApiError {
            status,
            message: error_text,
        }
        .into());
    }

    let completion_response = response
        .json::<ChatCompletionResponse>()
        .await
        .map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

    debug!(
        "Post completion response: {}",
        serde_json::to_string(&completion_response)
            .unwrap_or_else(|e| format!("<serialize error: {}>", e))
    );

    Ok(completion_response)
}
