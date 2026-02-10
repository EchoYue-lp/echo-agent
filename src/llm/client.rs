use std::sync::Arc;
use crate::error::{LlmError, Result};
use crate::llm::ChatCompletionRequest;
use crate::llm::types::ChatCompletionResponse;
use log::debug;
use reqwest::Client;
use reqwest::header::HeaderMap;

pub async fn post(
    client: Arc<Client>,
    request_body: &ChatCompletionRequest,
    header_map: HeaderMap,
    url: &str,
) -> Result<ChatCompletionResponse> {
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

    debug!("Post completion response: {:?}", completion_response);

    Ok(completion_response)
}
