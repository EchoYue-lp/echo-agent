use echo_core::error::{LlmError, Result};
use echo_core::llm::types::{ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse};
use echo_core::retry::{RetryPolicy, with_retry_if};
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, trace};

/// Determine whether an LLM error is retryable (network error / 429 rate limit / 5xx server error)
fn is_retryable(err: &LlmError) -> bool {
    match err {
        LlmError::NetworkError(_) => true,
        LlmError::ApiError { status, .. } => *status == 429 || *status >= 500,
        _ => false,
    }
}

fn env_duration_ms(name: &str, default_ms: u64) -> Option<Duration> {
    let ms = std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default_ms);
    (ms > 0).then(|| Duration::from_millis(ms))
}

fn timeout_error(kind: &str, duration: Duration) -> LlmError {
    LlmError::NetworkError(format!(
        "LLM stream {kind} timeout after {}ms",
        duration.as_millis()
    ))
}

fn split_sse_event(buffer: &mut String) -> Option<String> {
    let lf = buffer.find("\n\n");
    let crlf = buffer.find("\r\n\r\n");
    let (pos, sep_len) = match (lf, crlf) {
        (Some(a), Some(b)) if a <= b => (a, 2),
        (Some(_), Some(b)) => (b, 4),
        (Some(a), None) => (a, 2),
        (None, Some(b)) => (b, 4),
        (None, None) => return None,
    };
    let event = buffer[..pos].to_string();
    buffer.replace_range(..pos + sep_len, "");
    Some(event)
}

fn parse_sse_data(event: &str) -> Option<String> {
    let mut data_lines = Vec::new();
    for raw_line in event.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty()
            || line.starts_with(':')
            || line.starts_with("event:")
            || line.starts_with("id:")
            || line.starts_with("retry:")
        {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.strip_prefix(' ').unwrap_or(data).to_string());
        }
    }
    if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    }
}

const STREAM_DONE_SENTINEL: &str = "__ECHO_AGENT_STREAM_DONE__";

fn parse_sse_chunk(data: &str) -> Option<Result<ChatCompletionChunk>> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "[DONE]" {
        return Some(Err(LlmError::NetworkError(
            STREAM_DONE_SENTINEL.to_string(),
        )
        .into()));
    }
    match serde_json::from_str::<ChatCompletionChunk>(trimmed) {
        Ok(chunk) => {
            // Log when the final chunk includes usage so we can verify
            // the provider's stream_options.include_usage support.
            if chunk.usage.is_some() {
                tracing::debug!(
                    has_choices = !chunk.choices.is_empty(),
                    "SSE chunk with usage parsed successfully"
                );
            }
            Some(Ok(chunk))
        }
        Err(e) => {
            // Warn-level so the user can see if the provider returns
            // non-standard chunks that fail to parse as ChatCompletionChunk
            // (e.g. usage-only chunks with unexpected field types).
            tracing::warn!(error = %e, data = %trimmed, "SSE chunk failed to parse — provider may use non-standard format");
            None
        }
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
        "Stream completion: model={}, url={}, first_chunk_timeout_ms={:?}, idle_timeout_ms={:?}, overall_timeout_ms={:?}",
        request_body.model,
        url,
        env_duration_ms("ECHO_AGENT_STREAM_FIRST_CHUNK_TIMEOUT_MS", 30_000).map(|d| d.as_millis()),
        env_duration_ms("ECHO_AGENT_STREAM_IDLE_TIMEOUT_MS", 60_000).map(|d| d.as_millis()),
        env_duration_ms("ECHO_AGENT_STREAM_OVERALL_TIMEOUT_MS", 0).map(|d| d.as_millis())
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
    let first_chunk_timeout = env_duration_ms("ECHO_AGENT_STREAM_FIRST_CHUNK_TIMEOUT_MS", 30_000);
    let idle_timeout = env_duration_ms("ECHO_AGENT_STREAM_IDLE_TIMEOUT_MS", 60_000);
    let overall_timeout = env_duration_ms("ECHO_AGENT_STREAM_OVERALL_TIMEOUT_MS", 0);

    let stream = async_stream::try_stream! {
        let mut buffer = String::new();
        let mut received_any_bytes = false;
        let overall_sleep = overall_timeout.map(tokio::time::sleep);
        tokio::pin!(byte_stream);
        tokio::pin!(overall_sleep);

        loop {
            if let Some(ref ct) = cancel_token
                && ct.is_cancelled() {
                    tracing::info!("Stream cancelled by caller");
                    return;
                }

            let next_bytes = byte_stream.next();
            tokio::pin!(next_bytes);

            let bytes = tokio::select! {
                _ = async {
                    if let Some(sleep) = overall_sleep.as_mut().as_pin_mut() {
                        sleep.await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    if let Some(timeout) = overall_timeout {
                        tracing::warn!(
                            model = %request_body.model,
                            timeout_ms = timeout.as_millis() as u64,
                            "LLM stream overall timeout"
                        );
                        Err(timeout_error("overall", timeout))
                    } else {
                        unreachable!()
                    }
                }
                result = async {
                    let timeout = if received_any_bytes { idle_timeout } else { first_chunk_timeout };
                    if let Some(duration) = timeout {
                        match tokio::time::timeout(duration, next_bytes).await {
                            Ok(result) => Ok(result),
                            Err(_) => {
                                tracing::warn!(
                                    model = %request_body.model,
                                    kind = if received_any_bytes { "idle" } else { "first chunk" },
                                    timeout_ms = duration.as_millis() as u64,
                                    "LLM stream timeout"
                                );
                                Err(timeout_error(if received_any_bytes { "idle" } else { "first chunk" }, duration))
                            }
                        }
                    } else {
                        Ok(next_bytes.await)
                    }
                } => result,
            };

            let bytes = bytes?;
            let Some(bytes) = bytes else {
                break;
            };
            let bytes = bytes.map_err(|e| LlmError::NetworkError(e.to_string()))?;
            received_any_bytes = true;
            tracing::debug!(
                model = %request_body.model,
                byte_len = bytes.len(),
                "LLM stream bytes received"
            );
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(event_str) = split_sse_event(&mut buffer) {
                if let Some(data) = parse_sse_data(&event_str)
                    && let Some(parsed) = parse_sse_chunk(&data) {
                        match parsed {
                            Ok(chunk) => {
                                tracing::debug!(
                                    model = %request_body.model,
                                    choice_count = chunk.choices.len(),
                                    "LLM stream SSE chunk parsed"
                                );
                                yield chunk
                            },
                            Err(err) if err.to_string().contains(STREAM_DONE_SENTINEL) => return,
                            Err(err) => Err(err)?,
                        }
                    }
            }
        }

        if let Some(data) = parse_sse_data(&buffer)
            && let Some(parsed) = parse_sse_chunk(&data) {
                match parsed {
                    Ok(chunk) => {
                        tracing::debug!(
                            model = %request_body.model,
                            choice_count = chunk.choices.len(),
                            "LLM stream final buffered SSE chunk parsed"
                        );
                        yield chunk
                    },
                    Err(err) if err.to_string().contains("__ECHO_AGENT_STREAM_DONE__") => return,
                    Err(err) => Err(err)?,
                }
            }
    };

    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_json(content: &str) -> String {
        format!(
            r#"{{"choices":[{{"delta":{{"content":"{}"}},"index":0}}]}}"#,
            content
        )
    }

    #[test]
    fn parse_data_without_space() {
        let event = format!("data:{}", chunk_json("hello"));
        let data = parse_sse_data(&event).unwrap();
        let chunk = parse_sse_chunk(&data).unwrap().unwrap();
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hello"));
    }

    #[test]
    fn parse_data_with_crlf_and_keepalive() {
        let mut buffer = format!(
            ": ping\r\nevent: message\r\ndata: {}\r\n\r\n",
            chunk_json("hi")
        );
        let event = split_sse_event(&mut buffer).unwrap();
        let data = parse_sse_data(&event).unwrap();
        let chunk = parse_sse_chunk(&data).unwrap().unwrap();
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hi"));
        assert!(buffer.is_empty());
    }

    #[test]
    fn parse_done_marker() {
        let data = parse_sse_data("data: [DONE]").unwrap();
        let parsed = parse_sse_chunk(&data).unwrap();
        assert!(parsed.is_err());
    }
}
