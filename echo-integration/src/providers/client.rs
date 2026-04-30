use echo_core::error::{LlmError, Result};
use echo_core::llm::types::{ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse};
use echo_core::retry::{RetryPolicy, with_retry_if};
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use tracing::{info, trace};

/// Determine whether an LLM error is retryable (network error / 429 rate limit / 5xx server error)
fn is_retryable(err: &LlmError) -> bool {
    match err {
        LlmError::NetworkError(_) => true,
        LlmError::ApiError { status, .. } => *status == 429 || *status >= 500,
        _ => false,
    }
}

#[tracing::instrument(skip(client, request_body, header_map), fields(model = %request_body.model))]
pub async fn post(
    client: Arc<Client>,
    request_body: &ChatCompletionRequest,
    header_map: HeaderMap,
    url: &str,
) -> Result<ChatCompletionResponse> {
    trace!(
        model = %request_body.model,
        message_count = request_body.messages.len(),
        "Post completion request"
    );

    let policy = RetryPolicy::default();
    let response = with_retry_if(
        &policy,
        || {
            let client = client.clone();
            let header_map = header_map.clone();
            async move {
                let response = client
                    .post(url)
                    .headers(header_map)
                    .json(request_body)
                    .send()
                    .await
                    .map_err(|e| LlmError::NetworkError(e.to_string()))?;

                if !response.status().is_success() {
                    let status = response.status().as_u16();
                    let error_text = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "Unknown error".to_string());
                    return Err(LlmError::ApiError {
                        status,
                        message: error_text,
                    });
                }

                Ok(response)
            }
        },
        is_retryable,
    )
    .await?;

    let raw_text = response
        .text()
        .await
        .map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

    tracing::debug!(raw_len = raw_text.len(), raw = %raw_text.chars().take(2000).collect::<String>(), "Raw API response");

    let completion_response: ChatCompletionResponse =
        serde_json::from_str(&raw_text).map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

    trace!(
        choice_count = completion_response.choices.len(),
        "Post completion response received"
    );

    Ok(completion_response)
}

/// Send a request with `stream: true`, returning a parsed SSE chunk stream.
///
/// Note: Takes ownership of `request_body` to avoid lifetime conflicts between
/// references and the async stream.
///
/// `cancel_token` enables aborting the stream: the cancellation signal is checked
/// between each SSE chunk, and iteration stops immediately once cancelled.
#[tracing::instrument(skip(client, request_body, header_map, url, cancel_token), fields(model = %request_body.model))]
pub async fn stream_post(
    client: Arc<Client>,
    request_body: ChatCompletionRequest,
    header_map: HeaderMap,
    url: String,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> Result<impl Stream<Item = Result<ChatCompletionChunk>>> {
    info!(
        "Stream completion: model={}, url={}",
        request_body.model, url
    );
    trace!(
        model = %request_body.model,
        message_count = request_body.messages.len(),
        "Stream completion request"
    );

    let policy = RetryPolicy::default();
    let response = with_retry_if(
        &policy,
        || {
            let client = client.clone();
            let header_map = header_map.clone();
            let url = url.clone();
            let request_body = request_body.clone();
            async move {
                let response = client
                    .post(&url)
                    .headers(header_map)
                    .json(&request_body)
                    .send()
                    .await
                    .map_err(|e| LlmError::NetworkError(e.to_string()))?;

                if !response.status().is_success() {
                    let status = response.status().as_u16();
                    let error_text = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "Unknown error".to_string());
                    return Err(LlmError::ApiError {
                        status,
                        message: error_text,
                    });
                }

                Ok(response)
            }
        },
        is_retryable,
    )
    .await?;

    let byte_stream = response.bytes_stream();

    let stream = async_stream::try_stream! {
        let mut buffer = String::new();
        tokio::pin!(byte_stream);

        while let Some(bytes) = byte_stream.next().await {
            // 检查取消信号
            if let Some(ref ct) = cancel_token
                && ct.is_cancelled() {
                    tracing::info!("Stream cancelled by caller");
                    return;
                }

            let bytes = bytes.map_err(|e| LlmError::NetworkError(e.to_string()))?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            // 按 SSE 双换行切割完整事件
            while let Some(pos) = buffer.find("\n\n") {
                let event_str = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                for line in event_str.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data.trim() == "[DONE]" {
                            return;
                        }
                        match serde_json::from_str::<ChatCompletionChunk>(data) {
                            Ok(chunk) => yield chunk,
                            Err(e) => {
                                // Some providers mix in non-standard lines; skip them
                                tracing::debug!("skip non-standard SSE line: {} — {}", e, data);
                            }
                        }
                    }
                }
            }
        }

        // Handle trailing residual data (some services don't end with \n\n)
        for line in buffer.lines() {
            if let Some(data) = line.strip_prefix("data: ") &&
                 data.trim() != "[DONE]" &&
                     let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) {
                        yield chunk;
                    }
            }
    };

    Ok(stream)
}
