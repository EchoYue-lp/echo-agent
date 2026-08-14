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

#[cfg(test)]
enum ParsedSseChunk {
    Done,
    Chunk(ChatCompletionChunk),
}

/// Provider-neutral payload decoded from one SSE event.
pub(crate) enum JsonSseEvent {
    /// Compatibility terminator used by Chat Completions streams.
    Done,
    /// Semantic JSON event payload.
    Data(serde_json::Value),
}

fn parse_json_sse_event(data: &str) -> Result<Option<JsonSseEvent>> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed == "[DONE]" {
        return Ok(Some(JsonSseEvent::Done));
    }
    serde_json::from_str(trimmed)
        .map(JsonSseEvent::Data)
        .map(Some)
        .map_err(|error| LlmError::InvalidResponse(format!("invalid SSE JSON: {error}")).into())
}

#[cfg(test)]
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

    let value = post_json(
        client,
        serde_json::to_value(request_body)
            .map_err(|error| LlmError::InvalidResponse(error.to_string()))?,
        header_map,
        url,
    )
    .await?;
    let completion_response: ChatCompletionResponse = serde_json::from_value(value)
        .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;

    trace!(
        choice_count = completion_response.choices.len(),
        "Post completion response received"
    );

    Ok(completion_response)
}

/// Send a JSON request and return the complete JSON response body.
pub(crate) async fn post_json(
    client: Arc<Client>,
    request_body: serde_json::Value,
    header_map: HeaderMap,
    url: &str,
) -> Result<serde_json::Value> {
    let response = client
        .post(url)
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
        }
        .into());
    }

    let raw_text = response
        .text()
        .await
        .map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

    tracing::debug!(raw_len = raw_text.len(), raw = %raw_text.chars().take(2000).collect::<String>(), "Raw API response");

    serde_json::from_str(&raw_text)
        .map_err(|error| LlmError::InvalidResponse(error.to_string()).into())
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
    let model = request_body.model.clone();
    let body = serde_json::to_value(request_body)
        .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
    let raw_stream = stream_json_sse(client, body, header_map, url, model, cancel_token).await?;
    Ok(async_stream::try_stream! {
        futures::pin_mut!(raw_stream);
        while let Some(event) = raw_stream.next().await {
            match event? {
                JsonSseEvent::Done => return,
                JsonSseEvent::Data(value) => {
                    let chunk = serde_json::from_value::<ChatCompletionChunk>(value)
                        .map_err(|error| LlmError::InvalidResponse(format!("invalid Chat Completions SSE event: {error}")))?;
                    yield chunk;
                }
            }
        }
    })
}

/// Send a JSON request and decode its SSE response without assuming a provider
/// event schema. Chat Completions and Responses share this transport while
/// retaining independent wire adapters.
pub(crate) async fn stream_json_sse(
    client: Arc<Client>,
    request_body: serde_json::Value,
    header_map: HeaderMap,
    url: String,
    model: String,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> Result<impl Stream<Item = Result<JsonSseEvent>>> {
    info!(
        "Stream completion: model={}, url={}, first_chunk_timeout_ms={:?}, idle_timeout_ms={:?}, overall_timeout_ms={:?}",
        model,
        url,
        env_duration_ms("ECHO_AGENT_STREAM_FIRST_CHUNK_TIMEOUT_MS", 30_000).map(|d| d.as_millis()),
        env_duration_ms("ECHO_AGENT_STREAM_IDLE_TIMEOUT_MS", 60_000).map(|d| d.as_millis()),
        env_duration_ms("ECHO_AGENT_STREAM_OVERALL_TIMEOUT_MS", 0).map(|d| d.as_millis())
    );

    let request_future = async {
        let response = client
            .post(&url)
            .headers(header_map)
            .json(&request_body)
            .send()
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(LlmError::ApiError { status, message });
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

    Ok(async_stream::try_stream! {
        let mut decoder = SseDecoder::new();
        decoder.push(&first_bytes)?;
        while let Some(event) = decoder.next_event() {
            let parsed = parse_sse_data(&event)
                .map(|data| parse_json_sse_event(&data))
                .transpose()?
                .flatten();
            if let Some(parsed) = parsed {
                let done = matches!(parsed, JsonSseEvent::Done);
                yield parsed;
                if done {
                    return;
                }
            }
        }

        let overall_sleep = overall_timeout.map(tokio::time::sleep);
        tokio::pin!(overall_sleep);
        loop {
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
                } => match overall_timeout {
                    Some(timeout) => Err(timeout_error("overall", timeout)),
                    None => Err(LlmError::NetworkError("LLM stream timeout completed unexpectedly".to_string())),
                },
                result = async {
                    match idle_timeout {
                        Some(duration) => tokio::time::timeout(duration, next_bytes)
                            .await
                            .map_err(|_| timeout_error("idle", duration)),
                        None => Ok(next_bytes.await),
                    }
                } => result,
            }?;
            let Some(bytes) = bytes else {
                break;
            };
            let bytes = bytes.map_err(|error| LlmError::NetworkError(error.to_string()))?;
            decoder.push(&bytes)?;
            while let Some(event) = decoder.next_event() {
                let parsed = parse_sse_data(&event)
                    .map(|data| parse_json_sse_event(&data))
                    .transpose()?
                    .flatten();
                if let Some(parsed) = parsed {
                    let done = matches!(parsed, JsonSseEvent::Done);
                    yield parsed;
                    if done {
                        return;
                    }
                }
            }
        }

        if let Some(event) = decoder.finish()? {
            let data = parse_sse_data(&event).ok_or_else(|| {
                LlmError::InvalidResponse("truncated SSE event at EOF".to_string())
            })?;
            if let Some(parsed) = parse_json_sse_event(&data)? {
                yield parsed;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

    fn chunk_json(content: &str) -> String {
        format!(
            r#"{{"choices":[{{"delta":{{"content":"{}"}},"index":0}}]}}"#,
            content
        )
    }

    #[test]
    fn parse_data_without_space() -> Result<()> {
        let event = format!("data:{}", chunk_json("hello"));
        let data = parse_sse_data(&event)
            .ok_or_else(|| LlmError::InvalidResponse("SSE data line was not parsed".to_string()))?;
        let Some(ParsedSseChunk::Chunk(chunk)) = parse_sse_chunk(&data)? else {
            return Err(LlmError::InvalidResponse("SSE chunk was not parsed".to_string()).into());
        };
        assert_eq!(
            chunk
                .choices
                .first()
                .and_then(|choice| choice.delta.content.as_deref()),
            Some("hello")
        );
        Ok(())
    }

    #[test]
    fn parse_data_with_crlf_and_keepalive() -> Result<()> {
        let mut buffer = format!(
            ": ping\r\nevent: message\r\ndata: {}\r\n\r\n",
            chunk_json("hi")
        );
        let event = split_sse_event(&mut buffer).ok_or_else(|| {
            LlmError::InvalidResponse("SSE event boundary was not parsed".to_string())
        })?;
        let data = parse_sse_data(&event)
            .ok_or_else(|| LlmError::InvalidResponse("SSE data line was not parsed".to_string()))?;
        let Some(ParsedSseChunk::Chunk(chunk)) = parse_sse_chunk(&data)? else {
            return Err(LlmError::InvalidResponse("SSE chunk was not parsed".to_string()).into());
        };
        assert_eq!(
            chunk
                .choices
                .first()
                .and_then(|choice| choice.delta.content.as_deref()),
            Some("hi")
        );
        assert!(buffer.is_empty());
        Ok(())
    }

    #[test]
    fn parse_done_marker() -> Result<()> {
        let data = parse_sse_data("data: [DONE]").ok_or_else(|| {
            LlmError::InvalidResponse("SSE done marker was not parsed".to_string())
        })?;
        assert!(matches!(
            parse_sse_chunk(&data),
            Ok(Some(ParsedSseChunk::Done))
        ));
        Ok(())
    }

    #[test]
    fn decoder_preserves_split_multibyte_utf8() -> Result<()> {
        let payload = "data: {\"text\":\"你好\"}\n\n".as_bytes();
        let split = payload
            .iter()
            .position(|byte| *byte >= 0x80)
            .ok_or_else(|| {
                LlmError::InvalidResponse("test payload contained no multibyte UTF-8".to_string())
            })?
            .saturating_add(1);
        let mut decoder = SseDecoder::new();
        let first = payload.get(..split).ok_or_else(|| {
            LlmError::InvalidResponse("test split exceeded payload length".to_string())
        })?;
        decoder.push(first)?;
        assert!(decoder.next_event().is_none());
        let second = payload.get(split..).ok_or_else(|| {
            LlmError::InvalidResponse("test split exceeded payload length".to_string())
        })?;
        decoder.push(second)?;
        assert_eq!(
            decoder.next_event().as_deref(),
            Some("data: {\"text\":\"你好\"}")
        );
        assert!(decoder.finish()?.is_none());
        Ok(())
    }

    #[test]
    fn decoder_rejects_truncated_multibyte_utf8() -> Result<()> {
        let mut decoder = SseDecoder::new();
        decoder.push(&[0xe4])?;
        assert!(decoder.finish().is_err());
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_interrupts_parked_byte_stream() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let parked = Arc::new(Notify::new());
        let server_parked = parked.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = vec![0_u8; 2048];
            let _request_bytes = socket.read(&mut request).await?;
            let payload = b"data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"first\"}\n\n";
            let headers = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n";
            socket.write_all(headers).await?;
            let chunk_header = format!("{:X}\r\n", payload.len());
            socket.write_all(chunk_header.as_bytes()).await?;
            socket.write_all(payload).await?;
            socket.write_all(b"\r\n").await?;
            socket.flush().await?;
            server_parked.notify_one();
            std::future::pending::<std::io::Result<()>>().await
        });

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let stream = stream_json_sse(
            Arc::new(Client::new()),
            serde_json::json!({"model": "test", "stream": true}),
            HeaderMap::new(),
            format!("http://{address}"),
            "test".to_string(),
            Some(cancel_token.clone()),
        )
        .await?;
        futures::pin_mut!(stream);

        tokio::time::timeout(Duration::from_secs(2), parked.notified())
            .await
            .map_err(|_| LlmError::NetworkError("test stream did not park".to_string()))?;
        let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .map_err(|_| LlmError::NetworkError("first SSE event timed out".to_string()))?
            .ok_or_else(|| {
                LlmError::InvalidResponse("stream ended before first SSE event".to_string())
            })??;
        let JsonSseEvent::Data(first) = first else {
            return Err(LlmError::InvalidResponse(
                "stream returned done before first SSE event".to_string(),
            )
            .into());
        };
        assert_eq!(
            first.get("delta").and_then(serde_json::Value::as_str),
            Some("first")
        );

        let next_event = stream.next();
        tokio::pin!(next_event);
        let cancel_after_poll = async {
            tokio::task::yield_now().await;
            cancel_token.cancel();
        };
        let cancelled = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                result = &mut next_event => result,
                _ = cancel_after_poll => next_event.await,
            }
        })
        .await
        .map_err(|_| LlmError::NetworkError("parked SSE cancellation timed out".to_string()))?
        .ok_or_else(|| {
            LlmError::InvalidResponse("stream ended without surfacing cancellation".to_string())
        })?;
        assert!(matches!(
            cancelled,
            Err(echo_core::error::ReactError::Llm(error))
                if matches!(error.as_ref(), LlmError::NetworkError(message) if message.contains("cancelled"))
        ));
        server.abort();
        Ok(())
    }
}
