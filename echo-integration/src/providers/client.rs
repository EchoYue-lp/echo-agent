use echo_core::error::{LlmError, Result};
use echo_core::llm::types::{ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse};
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, trace};

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

pub(crate) fn split_sse_event(buffer: &mut String) -> Option<String> {
    let lf = buffer.find("\n\n");
    let crlf = buffer.find("\r\n\r\n");
    let (pos, sep_len) = match (lf, crlf) {
        (Some(a), Some(b)) if a <= b => (a, 2),
        (Some(_), Some(b)) => (b, 4),
        (Some(a), None) => (a, 2),
        (None, Some(b)) => (b, 4),
        (None, None) => return None,
    };
    let event = buffer.get(..pos)?.to_string();
    let remaining = buffer.get(pos.saturating_add(sep_len)..)?.to_string();
    *buffer = remaining;
    Some(event)
}

pub(crate) fn parse_sse_data(event: &str) -> Option<String> {
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

/// Incremental SSE decoder shared by provider adapters. It preserves partial
/// UTF-8 code points and only exposes complete blank-line-delimited events.
pub(crate) struct SseDecoder {
    pending_bytes: Vec<u8>,
    buffer: String,
}

impl SseDecoder {
    pub(crate) fn new() -> Self {
        Self {
            pending_bytes: Vec::new(),
            buffer: String::new(),
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<()> {
        self.pending_bytes.extend_from_slice(bytes);
        match std::str::from_utf8(&self.pending_bytes) {
            Ok(text) => {
                self.buffer.push_str(text);
                self.pending_bytes.clear();
                Ok(())
            }
            Err(error) if error.error_len().is_none() => Ok(()),
            Err(error) => Err(LlmError::InvalidResponse(format!(
                "invalid UTF-8 in SSE stream: {error}"
            ))
            .into()),
        }
    }

    pub(crate) fn next_event(&mut self) -> Option<String> {
        split_sse_event(&mut self.buffer)
    }

    pub(crate) fn finish(mut self) -> Result<Option<String>> {
        if !self.pending_bytes.is_empty() {
            return Err(LlmError::InvalidResponse("truncated UTF-8 at SSE EOF".to_string()).into());
        }
        if self.buffer.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(std::mem::take(&mut self.buffer)))
        }
    }
}

enum ParsedSseChunk {
    Done,
    Chunk(ChatCompletionChunk),
}

fn parse_sse_chunk(data: &str) -> Result<Option<ParsedSseChunk>> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed == "[DONE]" {
        return Ok(Some(ParsedSseChunk::Done));
    }
    let chunk = serde_json::from_str::<ChatCompletionChunk>(trimmed)
        .map_err(|error| LlmError::InvalidResponse(format!("invalid SSE JSON: {error}")))?;
    if chunk.usage.is_some() {
        tracing::debug!(
            has_choices = !chunk.choices.is_empty(),
            "SSE chunk with usage parsed successfully"
        );
    }
    Ok(Some(ParsedSseChunk::Chunk(chunk)))
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
        }
        .into());
    }

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

    let request_future = async {
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
    };
    let response = tokio::select! {
        biased;
        _ = async {
            match cancel_token.as_ref() {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        } => return Err(LlmError::NetworkError("LLM stream cancelled".to_string()).into()),
        response = request_future => response?,
    };

    let mut byte_stream = Box::pin(response.bytes_stream());
    let first_chunk_timeout = env_duration_ms("ECHO_AGENT_STREAM_FIRST_CHUNK_TIMEOUT_MS", 30_000);
    let idle_timeout = env_duration_ms("ECHO_AGENT_STREAM_IDLE_TIMEOUT_MS", 60_000);
    let overall_timeout = env_duration_ms("ECHO_AGENT_STREAM_OVERALL_TIMEOUT_MS", 0);
    let first_bytes = tokio::select! {
        biased;
        _ = async {
            match cancel_token.as_ref() {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        } => return Err(LlmError::NetworkError("LLM stream cancelled".to_string()).into()),
        result = async {
            match first_chunk_timeout {
                Some(duration) => tokio::time::timeout(duration, byte_stream.next())
                    .await
                    .map_err(|_| timeout_error("first chunk", duration)),
                None => Ok(byte_stream.next().await),
            }
        } => result?,
    }
    .ok_or_else(|| {
        LlmError::InvalidResponse("LLM stream ended before the first chunk".to_string())
    })?
    .map_err(|error| LlmError::NetworkError(error.to_string()))?;

    let stream = async_stream::try_stream! {
        let mut decoder = SseDecoder::new();
        decoder.push(&first_bytes)?;
        while let Some(event_str) = decoder.next_event() {
            if let Some(data) = parse_sse_data(&event_str) {
                match parse_sse_chunk(&data)? {
                    Some(ParsedSseChunk::Chunk(chunk)) => yield chunk,
                    Some(ParsedSseChunk::Done) => return,
                    None => {}
                }
            }
        }
        let overall_sleep = overall_timeout.map(tokio::time::sleep);
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
                biased;
                _ = async {
                    match cancel_token.as_ref() {
                        Some(token) => token.cancelled().await,
                        None => std::future::pending().await,
                    }
                } => Err(LlmError::NetworkError("LLM stream cancelled".to_string())),
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
                        Err(LlmError::NetworkError(
                            "LLM stream timeout future completed without a configured timeout"
                                .to_string(),
                        ))
                    }
                }
                result = async {
                    if let Some(duration) = idle_timeout {
                        match tokio::time::timeout(duration, next_bytes).await {
                            Ok(result) => Ok(result),
                            Err(_) => {
                                tracing::warn!(
                                    model = %request_body.model,
                                    kind = "idle",
                                    timeout_ms = duration.as_millis() as u64,
                                    "LLM stream timeout"
                                );
                                Err(timeout_error("idle", duration))
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
            tracing::debug!(
                model = %request_body.model,
                byte_len = bytes.len(),
                "LLM stream bytes received"
            );
            decoder.push(&bytes)?;

            while let Some(event_str) = decoder.next_event() {
                if let Some(data) = parse_sse_data(&event_str) {
                    match parse_sse_chunk(&data)? {
                        Some(ParsedSseChunk::Chunk(chunk)) => yield chunk,
                        Some(ParsedSseChunk::Done) => return,
                        None => {}
                    }
                }
            }
        }

        if let Some(event) = decoder.finish()? {
            let data = parse_sse_data(&event).ok_or_else(|| {
                LlmError::InvalidResponse("truncated SSE event at EOF".to_string())
            })?;
            match parse_sse_chunk(&data)? {
                Some(ParsedSseChunk::Chunk(chunk)) => yield chunk,
                Some(ParsedSseChunk::Done) | None => return,
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
        let parsed = parse_sse_chunk(&data).ok().flatten();
        let Some(ParsedSseChunk::Chunk(chunk)) = parsed else {
            return;
        };
        assert_eq!(
            chunk
                .choices
                .first()
                .and_then(|choice| choice.delta.content.as_deref()),
            Some("hello")
        );
    }

    #[test]
    fn parse_data_with_crlf_and_keepalive() {
        let mut buffer = format!(
            ": ping\r\nevent: message\r\ndata: {}\r\n\r\n",
            chunk_json("hi")
        );
        let event = split_sse_event(&mut buffer).unwrap();
        let data = parse_sse_data(&event).unwrap();
        let parsed = parse_sse_chunk(&data).ok().flatten();
        let Some(ParsedSseChunk::Chunk(chunk)) = parsed else {
            return;
        };
        assert_eq!(
            chunk
                .choices
                .first()
                .and_then(|choice| choice.delta.content.as_deref()),
            Some("hi")
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn parse_done_marker() {
        let data = parse_sse_data("data: [DONE]").unwrap();
        assert!(matches!(
            parse_sse_chunk(&data),
            Ok(Some(ParsedSseChunk::Done))
        ));
    }

    #[test]
    fn decoder_preserves_split_multibyte_utf8() {
        let payload = "data: {\"text\":\"你好\"}\n\n".as_bytes();
        let split = payload
            .iter()
            .position(|byte| *byte >= 0x80)
            .unwrap_or_default()
            .saturating_add(1);
        let mut decoder = SseDecoder::new();
        decoder
            .push(payload.get(..split).unwrap_or_default())
            .unwrap();
        assert!(decoder.next_event().is_none());
        decoder
            .push(payload.get(split..).unwrap_or_default())
            .unwrap();
        assert_eq!(
            decoder.next_event().as_deref(),
            Some("data: {\"text\":\"你好\"}")
        );
        assert!(decoder.finish().unwrap().is_none());
    }

    #[test]
    fn decoder_rejects_truncated_multibyte_utf8() {
        let mut decoder = SseDecoder::new();
        decoder.push(&[0xe4]).unwrap();
        assert!(decoder.finish().is_err());
    }
}
