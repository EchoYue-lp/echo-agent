use crate::error::{LlmError, Result};
use crate::llm::ChatCompletionRequest;
use crate::llm::types::ChatCompletionResponse;
use reqwest::header::HeaderMap;

pub async fn post(
    request_body: &ChatCompletionRequest,
    header_map: HeaderMap,
    url: &str,
) -> Result<ChatCompletionResponse> {
    let client = reqwest::Client::new();

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

    println!("=============>\n {:?} \n================", completion_response);

    Ok(completion_response)
}
