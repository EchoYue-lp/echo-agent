use echo_core::error::{LlmError, Result};
use echo_core::llm::LlmTimeouts;
use echo_core::llm::types::{ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse};
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use reqwest::RequestBuilder;
use reqwest::header::HeaderMap;
use serde::de::DeserializeOwned;
use std::io::{Cursor, Read};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{info, trace};

fn timeout_error(kind: &str, duration: Duration) -> LlmError {
    LlmError::NetworkError(format!(
        "LLM stream {kind} timeout after {}ms",
        duration.as_millis()
    ))
}

fn request_cancelled_error() -> LlmError {
    LlmError::NetworkError("LLM request cancelled".to_string())
}

pub(crate) fn ensure_request_not_cancelled(cancel_token: Option<&CancellationToken>) -> Result<()> {
    if cancel_token.is_some_and(CancellationToken::is_cancelled) {
        Err(request_cancelled_error().into())
    } else {
        Ok(())
    }
}

async fn wait_for_deadline(
    timeout: Option<Duration>,
    deadline: Option<tokio::time::Instant>,
) -> Duration {
    match (timeout, deadline) {
        (Some(timeout), Some(deadline)) => {
            tokio::time::sleep_until(deadline).await;
            timeout
        }
        _ => std::future::pending().await,
    }
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

    pub(crate) fn finish(self) -> Result<Option<String>> {
        if !self.pending_bytes.is_empty() {
            return Err(LlmError::InvalidResponse("truncated UTF-8 at SSE EOF".to_string()).into());
        }
        // An SSE event is complete only after a blank-line delimiter.  A
        // delimiterless payload must never be reinterpreted as a valid event
        // merely because its JSON happens to be complete.
        if self.buffer.is_empty() {
            Ok(None)
        } else {
            Err(LlmError::InvalidResponse("truncated SSE event at EOF".to_string()).into())
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

#[tracing::instrument(skip(client, request_body, header_map, cancel_token), fields(model = %request_body.model))]
pub(crate) async fn post(
    client: Arc<Client>,
    request_body: &ChatCompletionRequest,
    header_map: HeaderMap,
    url: &str,
    timeouts: LlmTimeouts,
    cancel_token: Option<CancellationToken>,
) -> Result<ChatCompletionResponse> {
    trace!(
        model = %request_body.model,
        message_count = request_body.messages.len(),
        "Post completion request"
    );

    let completion_response: ChatCompletionResponse = post_json(
        client,
        serde_json::to_value(request_body)
            .map_err(|error| LlmError::InvalidResponse(error.to_string()))?,
        header_map,
        url,
        timeouts,
        cancel_token,
    )
    .await?;

    trace!(
        choice_count = completion_response.choices.len(),
        "Post completion response received"
    );

    Ok(completion_response)
}

/// Send a JSON request and return the complete JSON response body.
pub(crate) async fn post_json<T>(
    client: Arc<Client>,
    request_body: serde_json::Value,
    header_map: HeaderMap,
    url: &str,
    timeouts: LlmTimeouts,
    cancel_token: Option<CancellationToken>,
) -> Result<T>
where
    T: DeserializeOwned + Send + 'static,
{
    let request = client.post(url).headers(header_map).json(&request_body);
    let request = match timeouts.request_timeout() {
        Some(timeout) => request.timeout(timeout),
        None => request,
    };
    post_json_request(request, cancel_token).await
}

/// Send a request and decode its complete JSON response while honoring the
/// owning request cancellation token across the entire transport lifecycle.
///
/// The same boundary covers waiting for response headers, reading an error or
/// success body, and decoding JSON. Dropping the in-flight reqwest future on
/// cancellation closes the response body and releases the connection.
pub(crate) async fn post_json_request<T>(
    request: RequestBuilder,
    cancel_token: Option<CancellationToken>,
) -> Result<T>
where
    T: DeserializeOwned + Send + 'static,
{
    let transport_future = async {
        let response = request
            .send()
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response.bytes().await.map_or_else(
                |_| "Unknown error".to_string(),
                |bytes| String::from_utf8_lossy(&bytes).into_owned(),
            );
            return Err::<Vec<u8>, echo_core::error::ReactError>(
                LlmError::ApiError {
                    status,
                    message: error_text,
                }
                .into(),
            );
        }

        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| LlmError::InvalidResponse(error.to_string()).into())
    };
    tokio::pin!(transport_future);

    let transport_result = tokio::select! {
        biased;
        _ = async {
            match cancel_token.as_ref() {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        } => Err(request_cancelled_error().into()),
        result = &mut transport_future => result,
    };
    ensure_request_not_cancelled(cancel_token.as_ref())?;
    let raw_bytes = transport_result?;

    tracing::debug!(raw_len = raw_bytes.len(), "Raw API response received");

    decode_json_bytes(raw_bytes, cancel_token).await
}

struct CancellationReader<R> {
    inner: R,
    cancel_token: CancellationToken,
}

impl<R: Read> Read for CancellationReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel_token.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "LLM request cancelled during JSON decode",
            ));
        }
        let bounded_len = buffer.len().min(8 * 1024);
        let bounded = buffer.get_mut(..bounded_len).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "JSON decode buffer boundary was invalid",
            )
        })?;
        self.inner.read(bounded)
    }
}

fn decode_json_reader<T, R>(reader: R, cancel_token: CancellationToken) -> Result<T>
where
    T: DeserializeOwned,
    R: Read,
{
    let decoded = serde_json::from_reader(CancellationReader {
        inner: reader,
        cancel_token: cancel_token.clone(),
    });
    if cancel_token.is_cancelled() {
        return Err(request_cancelled_error().into());
    }
    decoded.map_err(|error| LlmError::InvalidResponse(error.to_string()).into())
}

async fn decode_json_bytes<T>(
    raw_bytes: Vec<u8>,
    cancel_token: Option<CancellationToken>,
) -> Result<T>
where
    T: DeserializeOwned + Send + 'static,
{
    let cancel_token = cancel_token.unwrap_or_default();
    if cancel_token.is_cancelled() {
        return Err(request_cancelled_error().into());
    }

    let decode_cancel = cancel_token.clone();
    let mut decode_task = tokio::task::spawn_blocking(move || {
        decode_json_reader(Cursor::new(raw_bytes), decode_cancel)
    });
    tokio::select! {
        biased;
        _ = cancel_token.cancelled() => {
            decode_task.abort();
            let _settled = decode_task.await;
            Err(request_cancelled_error().into())
        },
        result = &mut decode_task => {
            let decoded = result.map_err(|error| {
                LlmError::InvalidResponse(format!("JSON decode task failed: {error}"))
            })??;
            if cancel_token.is_cancelled() {
                Err(request_cancelled_error().into())
            } else {
                Ok(decoded)
            }
        },
    }
}

/// Send a request with `stream: true`, returning a parsed SSE chunk stream.
///
/// Note: Takes ownership of `request_body` to avoid lifetime conflicts between
/// references and the async stream.
///
/// `cancel_token` enables aborting the stream: the cancellation signal is checked
/// between each SSE chunk, and iteration stops immediately once cancelled.
/// A choice finish reason and usage are withheld until the final `[DONE]` marker.
#[tracing::instrument(skip(client, request_body, header_map, url, cancel_token), fields(model = %request_body.model))]
pub(crate) async fn stream_post(
    client: Arc<Client>,
    request_body: ChatCompletionRequest,
    header_map: HeaderMap,
    url: String,
    timeouts: LlmTimeouts,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> Result<impl Stream<Item = Result<ChatCompletionChunk>>> {
    let model = request_body.model.clone();
    let body = serde_json::to_value(request_body)
        .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
    let request = client.post(url).headers(header_map).json(&body);
    let raw_stream = stream_json_sse(request, model, timeouts, cancel_token).await?;
    Ok(async_stream::try_stream! {
        let mut terminal: Option<ChatCompletionChunk> = None;
        let mut stream_usage = None;
        futures::pin_mut!(raw_stream);
        while let Some(event) = raw_stream.next().await {
            match event? {
                JsonSseEvent::Done => {
                    let mut completed = terminal.ok_or_else(|| LlmError::InvalidResponse(
                        "Chat Completions stream ended with [DONE] before a finish reason".to_string()
                    ))?;
                    completed.usage = stream_usage.or(completed.usage);
                    yield completed;
                    return;
                }
                JsonSseEvent::Data(value) => {
                    let mut chunk = serde_json::from_value::<ChatCompletionChunk>(value)
                        .map_err(|error| LlmError::InvalidResponse(format!("invalid Chat Completions SSE event: {error}")))?;
                    if let Some(usage) = chunk.usage.take() {
                        stream_usage = Some(usage);
                    }
                    if terminal.is_some() {
                        if !chunk.choices.is_empty() {
                            Err(LlmError::InvalidResponse("Chat Completions emitted choices after its finish reason".to_string()))?;
                        }
                        continue;
                    }
                    let finish = chunk.choices.first().and_then(|choice| choice.finish_reason.as_deref());
                    if let Some(reason) = finish {
                        if !matches!(reason, "stop" | "tool_calls" | "function_call") {
                            Err(LlmError::InvalidResponse(format!(
                                "Chat Completions stream ended with non-success finish reason '{reason}'"
                            )))?;
                        }
                        let has_delta = chunk.choices.first().is_some_and(|choice| {
                            let delta = &choice.delta;
                            delta.role.is_some() || delta.content.is_some()
                                || delta.reasoning_content.is_some()
                                || delta.reasoning_blocks.is_some() || delta.tool_calls.is_some()
                        });
                        if has_delta {
                            let mut progress = chunk.clone();
                            if let Some(choice) = progress.choices.first_mut() {
                                choice.finish_reason = None;
                            }
                            progress.usage = None;
                            yield progress;
                        }
                        if let Some(choice) = chunk.choices.first_mut() {
                            choice.delta = Default::default();
                        }
                        terminal = Some(chunk);
                    } else if !chunk.choices.is_empty() {
                        yield chunk;
                    }
                }
            }
        }
        Err(LlmError::InvalidResponse(
            "Chat Completions stream reached SSE EOF before [DONE] and a finish reason".to_string()
        ))?;
    })
}

/// Send a JSON request and decode its SSE response without assuming a provider
/// event schema. Chat Completions, Responses, and Anthropic share this
/// transport while retaining independent semantic event adapters.
pub(crate) async fn stream_json_sse(
    request: RequestBuilder,
    model: String,
    timeouts: LlmTimeouts,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> Result<impl Stream<Item = Result<JsonSseEvent>>> {
    let first_chunk_timeout = timeouts.first_chunk_timeout();
    let idle_timeout = timeouts.idle_timeout();
    let overall_timeout = timeouts.overall_timeout();
    let overall_deadline = overall_timeout.map(|timeout| tokio::time::Instant::now() + timeout);
    info!(
        "Stream completion: model={}, first_chunk_timeout_ms={:?}, idle_timeout_ms={:?}, overall_timeout_ms={:?}",
        model,
        first_chunk_timeout.map(|duration| duration.as_millis()),
        idle_timeout.map(|duration| duration.as_millis()),
        overall_timeout.map(|duration| duration.as_millis())
    );

    let start_stream = async {
        let response = request
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
        let mut byte_stream = Box::pin(response.bytes_stream());
        let first_bytes = byte_stream
            .next()
            .await
            .ok_or_else(|| {
                LlmError::InvalidResponse("LLM stream ended before the first chunk".to_string())
            })?
            .map_err(|error| LlmError::NetworkError(error.to_string()))?;
        Ok((byte_stream, first_bytes))
    };
    tokio::pin!(start_stream);
    let (mut byte_stream, first_bytes) = tokio::select! {
        biased;
        _ = async {
            match cancel_token.as_ref() {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        } => return Err(LlmError::NetworkError("LLM stream cancelled".to_string()).into()),
        duration = wait_for_deadline(overall_timeout, overall_deadline) => {
            return Err(timeout_error("overall", duration).into());
        },
        result = async {
            match first_chunk_timeout {
                Some(duration) => tokio::time::timeout(duration, &mut start_stream)
                    .await
                    .map_err(|_| timeout_error("first chunk", duration)),
                None => Ok((&mut start_stream).await),
            }
        } => result?,
    }?;

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
                duration = wait_for_deadline(overall_timeout, overall_deadline) => {
                    Err(timeout_error("overall", duration))
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
    use crate::providers::{AnthropicClient, LlmConfig, OpenAiClient, ResponsesClient};
    use echo_core::llm::{
        ChatRequest, LlmApiProtocol, LlmClient, Message, ModelInputModality, ThinkingProtocol,
    };
    use std::fmt::Debug;
    use std::sync::mpsc;
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

    #[test]
    fn decoder_rejects_delimiterless_event_at_eof() -> Result<()> {
        let mut decoder = SseDecoder::new();
        decoder.push(br#"data: {"text":"complete JSON but no SSE delimiter"}"#)?;

        let error = decoder.finish().err().ok_or_else(|| {
            LlmError::InvalidResponse("delimiterless SSE event was accepted".to_string())
        })?;
        assert!(matches!(
            error,
            echo_core::error::ReactError::Llm(inner)
                if matches!(inner.as_ref(), LlmError::InvalidResponse(message) if message == "truncated SSE event at EOF")
        ));
        Ok(())
    }

    #[test]
    fn decoder_rejects_whitespace_residue_at_eof() -> Result<()> {
        let mut decoder = SseDecoder::new();
        decoder.push(b" \n")?;
        assert!(matches!(
            decoder.finish(),
            Err(echo_core::error::ReactError::Llm(inner))
                if matches!(inner.as_ref(), LlmError::InvalidResponse(message) if message == "truncated SSE event at EOF")
        ));
        Ok(())
    }

    #[tokio::test]
    async fn first_chunk_timeout_covers_request_start_through_first_bytes() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = vec![0_u8; 2048];
            let _request_bytes = socket.read(&mut request).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                .await?;
            std::io::Result::Ok(())
        });
        let request = Client::new()
            .post(format!("http://{address}"))
            .json(&serde_json::json!({"model": "test", "stream": true}));
        let timeouts = LlmTimeouts::default()
            .with_first_chunk_timeout(Duration::from_millis(20))
            .without_overall_timeout();
        let result = stream_json_sse(request, "test".to_string(), timeouts, None).await;
        let message = result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(message.contains("first chunk timeout"));
        server.abort();
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
        let request = Client::new()
            .post(format!("http://{address}"))
            .json(&serde_json::json!({"model": "test", "stream": true}));
        let stream = stream_json_sse(
            request,
            "test".to_string(),
            LlmTimeouts::default(),
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

    fn assert_request_cancellation<T: Debug>(result: Result<T>) -> Result<()> {
        match result {
            Err(echo_core::error::ReactError::Llm(error)) if matches!(error.as_ref(), LlmError::NetworkError(message) if message.contains("cancelled")) => {
                Ok(())
            }
            Err(error) => Err(LlmError::InvalidResponse(format!(
                "expected typed network cancellation, got {error}"
            ))
            .into()),
            Ok(value) => Err(LlmError::InvalidResponse(format!(
                "request unexpectedly succeeded with {value:?}"
            ))
            .into()),
        }
    }

    #[derive(Clone, Copy)]
    enum TestProvider {
        OpenAi,
        Responses,
        Anthropic,
    }

    impl TestProvider {
        const ALL: [Self; 3] = [Self::OpenAi, Self::Responses, Self::Anthropic];

        fn name(self) -> &'static str {
            match self {
                Self::OpenAi => "openai",
                Self::Responses => "responses",
                Self::Anthropic => "anthropic",
            }
        }
    }

    fn test_provider_client(
        provider: TestProvider,
        base_url: String,
    ) -> Result<Box<dyn LlmClient>> {
        let timeouts = LlmTimeouts::default().without_request_timeout();
        match provider {
            TestProvider::OpenAi => {
                let config = LlmConfig {
                    provider_name: Some(provider.name().to_string()),
                    api_protocol: LlmApiProtocol::ChatCompletions,
                    base_url,
                    api_key: "test-key".to_string(),
                    model: "test-model".to_string(),
                    input_modalities: ModelInputModality::text_only(),
                    thinking_protocol: ThinkingProtocol::None,
                    timeouts,
                };
                Ok(Box::new(OpenAiClient::new(config)?))
            }
            TestProvider::Responses => {
                let config = LlmConfig {
                    provider_name: Some(provider.name().to_string()),
                    api_protocol: LlmApiProtocol::Responses,
                    base_url,
                    api_key: "test-key".to_string(),
                    model: "test-model".to_string(),
                    input_modalities: ModelInputModality::text_only(),
                    thinking_protocol: ThinkingProtocol::None,
                    timeouts,
                };
                Ok(Box::new(ResponsesClient::new(config)?))
            }
            TestProvider::Anthropic => Ok(Box::new(
                AnthropicClient::with_base_url(base_url, "test-key", "test-model")
                    .with_timeouts(timeouts),
            )),
        }
    }

    async fn collect_provider_sse(
        provider: TestProvider,
        events: &[&str],
    ) -> Result<Vec<Result<echo_core::llm::ChatChunk>>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let body = events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = vec![0_u8; 8192];
            let _request_bytes = socket.read(&mut request).await?;
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(headers.as_bytes()).await?;
            socket.write_all(body.as_bytes()).await?;
            socket.flush().await
        });
        let client = test_provider_client(provider, format!("http://{address}"))?;
        let stream = client
            .chat_stream(ChatRequest {
                messages: vec![Message::user("hello".to_string())],
                ..Default::default()
            })
            .await?;
        let items = tokio::time::timeout(Duration::from_secs(2), stream.collect::<Vec<_>>())
            .await
            .map_err(|_| LlmError::NetworkError("provider fixture timed out".to_string()))?;
        server
            .await
            .map_err(|error| LlmError::NetworkError(format!("fixture failed: {error}")))??;
        Ok(items)
    }

    #[tokio::test]
    async fn provider_streams_require_semantic_completion_after_partial_output() -> Result<()> {
        let cases: &[(TestProvider, &[&str])] = &[
            (
                TestProvider::OpenAi,
                &[r#"{"choices":[{"delta":{"content":"partial"},"index":0}]}"#],
            ),
            (
                TestProvider::OpenAi,
                &[
                    r#"{"choices":[{"delta":{"content":"partial"},"index":0}]}"#,
                    r#"{"choices":[{"delta":null,"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#,
                ],
            ),
            (
                TestProvider::Anthropic,
                &[
                    r#"{"type":"message_start","message":{"usage":{"input_tokens":2,"output_tokens":0}}}"#,
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}"#,
                    r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}"#,
                ],
            ),
            (
                TestProvider::Responses,
                &[
                    r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"partial"}"#,
                ],
            ),
        ];
        for (provider, events) in cases {
            let items = collect_provider_sse(*provider, events).await?;
            assert!(
                items.iter().any(|item| item
                    .as_ref()
                    .ok()
                    .and_then(|chunk| chunk.delta.content.as_ref())
                    .is_some()),
                "{} lost partial output",
                provider.name()
            );
            assert!(items.iter().any(|item| matches!(item, Err(echo_core::error::ReactError::Llm(error)) if matches!(error.as_ref(), LlmError::InvalidResponse(_)))), "{} accepted missing semantic terminal", provider.name());
            assert!(
                !items.iter().any(|item| item
                    .as_ref()
                    .ok()
                    .and_then(|chunk| chunk.finish_reason.as_ref())
                    .is_some()),
                "{} published a finish before semantic terminal",
                provider.name()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn provider_streams_complete_with_one_terminal_and_usage() -> Result<()> {
        let cases: &[(TestProvider, &[&str])] = &[
            (
                TestProvider::OpenAi,
                &[
                    r#"{"choices":[{"delta":{"content":"done"},"index":0}]}"#,
                    r#"{"choices":[{"delta":null,"index":0,"finish_reason":"stop"}]}"#,
                    r#"{"choices":[],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#,
                    "[DONE]",
                ],
            ),
            (
                TestProvider::Anthropic,
                &[
                    r#"{"type":"message_start","message":{"usage":{"input_tokens":2,"output_tokens":0}}}"#,
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"done"}}"#,
                    r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}"#,
                    r#"{"type":"message_stop"}"#,
                ],
            ),
            (
                TestProvider::Responses,
                &[
                    r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"done"}"#,
                    r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[],"usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}}"#,
                ],
            ),
        ];
        for (provider, events) in cases {
            let items = collect_provider_sse(*provider, events).await?;
            assert!(
                items.iter().all(Result::is_ok),
                "{} rejected a completed stream",
                provider.name()
            );
            let terminals = items
                .iter()
                .filter_map(|item| item.as_ref().ok())
                .filter(|chunk| chunk.finish_reason.is_some())
                .collect::<Vec<_>>();
            assert_eq!(
                terminals.len(),
                1,
                "{} must emit one terminal",
                provider.name()
            );
            assert_eq!(
                terminals
                    .first()
                    .and_then(|chunk| chunk.finish_reason.as_deref()),
                Some("stop")
            );
            assert_eq!(
                terminals
                    .first()
                    .and_then(|chunk| chunk.usage.as_ref())
                    .and_then(|usage| usage.total_tokens),
                Some(3)
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn provider_streams_reject_incomplete_or_invalid_terminal_signals() -> Result<()> {
        let cases: &[(TestProvider, &[&str])] = &[
            (
                TestProvider::OpenAi,
                &[
                    r#"{"choices":[{"delta":{"content":"partial"},"index":0}]}"#,
                    "[DONE]",
                ],
            ),
            (
                TestProvider::OpenAi,
                &[
                    r#"{"choices":[{"delta":{"content":"partial"},"index":0}]}"#,
                    r#"{"choices":[{"delta":null,"index":0,"finish_reason":"length"}]}"#,
                    "[DONE]",
                ],
            ),
            (
                TestProvider::OpenAi,
                &[
                    r#"{"choices":[{"delta":null,"index":0,"finish_reason":"stop"}]}"#,
                    r#"{"choices":[{"delta":{"content":"late"},"index":0}]}"#,
                    "[DONE]",
                ],
            ),
            (
                TestProvider::Anthropic,
                &[
                    r#"{"type":"message_start","message":{"usage":{"input_tokens":2,"output_tokens":0}}}"#,
                    r#"{"type":"message_stop"}"#,
                ],
            ),
            (
                TestProvider::Anthropic,
                &[
                    r#"{"type":"message_start","message":{"usage":{"input_tokens":2,"output_tokens":0}}}"#,
                    r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":1}}"#,
                    r#"{"type":"message_stop"}"#,
                ],
            ),
            (TestProvider::Responses, &["[DONE]"]),
        ];
        for (provider, events) in cases {
            let items = collect_provider_sse(*provider, events).await?;
            assert!(items.iter().any(|item| matches!(item, Err(echo_core::error::ReactError::Llm(error)) if matches!(error.as_ref(), LlmError::InvalidResponse(_)))), "{} accepted an invalid semantic terminal", provider.name());
            assert!(
                !items.iter().any(|item| item
                    .as_ref()
                    .ok()
                    .and_then(|chunk| chunk.finish_reason.as_ref())
                    .is_some()),
                "{} published a successful finish for an invalid terminal",
                provider.name()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn openai_usage_before_done_is_not_published_on_truncated_stream() -> Result<()> {
        let items = collect_provider_sse(
            TestProvider::OpenAi,
            &[
                r#"{"choices":[{"delta":{"content":"partial"},"index":0}]}"#,
                r#"{"choices":[{"delta":null,"index":0,"finish_reason":"stop"}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#,
            ],
        ).await?;
        assert!(items.iter().any(Result::is_err));
        assert!(!items.iter().any(|item| {
            item.as_ref()
                .ok()
                .is_some_and(|chunk| chunk.finish_reason.is_some() || chunk.usage.is_some())
        }));
        Ok(())
    }

    async fn assert_provider_cancels_while_stalled(
        provider: TestProvider,
        after_headers: bool,
    ) -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let stalled = Arc::new(Notify::new());
        let server_stalled = stalled.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = vec![0_u8; 4096];
            let _request_bytes = socket.read(&mut request).await?;
            if after_headers {
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 64\r\nConnection: keep-alive\r\n\r\n",
                    )
                    .await?;
                socket.flush().await?;
            }
            server_stalled.notify_one();
            std::future::pending::<std::io::Result<()>>().await
        });

        let client = test_provider_client(provider, format!("http://{address}"))?;
        let cancel_token = CancellationToken::new();
        let cancel_after_stall = cancel_token.clone();
        let cancel_task = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(2), stalled.notified())
                .await
                .map_err(|_| {
                    LlmError::NetworkError("provider request did not stall".to_string())
                })?;
            cancel_after_stall.cancel();
            Result::<()>::Ok(())
        });
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            client.chat(ChatRequest {
                messages: vec![Message::user("hello".to_string())],
                cancel_token: Some(cancel_token),
                timeouts: Some(LlmTimeouts::default().without_request_timeout()),
                ..Default::default()
            }),
        )
        .await
        .map_err(|_| {
            LlmError::NetworkError(format!(
                "{} non-stream cancellation timed out",
                provider.name()
            ))
        })?;
        cancel_task
            .await
            .map_err(|error| LlmError::NetworkError(format!("cancel task failed: {error}")))??;
        assert_request_cancellation(result)?;
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn all_nonstream_providers_cancel_before_response_headers() -> Result<()> {
        for provider in TestProvider::ALL {
            assert_provider_cancels_while_stalled(provider, false).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn all_nonstream_providers_cancel_after_response_headers() -> Result<()> {
        for provider in TestProvider::ALL {
            assert_provider_cancels_while_stalled(provider, true).await?;
        }
        Ok(())
    }

    struct PausingReader {
        inner: Cursor<Vec<u8>>,
        started: Option<tokio::sync::oneshot::Sender<()>>,
        resume: mpsc::Receiver<()>,
    }

    impl Read for PausingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if let Some(started) = self.started.take() {
                started.send(()).map_err(|()| {
                    std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "decode-start observer was dropped",
                    )
                })?;
                self.resume.recv().map_err(|error| {
                    std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        format!("decode resume sender was dropped: {error}"),
                    )
                })?;
            }
            std::io::Read::read(&mut self.inner, buffer)
        }
    }

    #[tokio::test]
    async fn cancellation_wins_after_json_decode_has_started() -> Result<()> {
        let cancel_token = CancellationToken::new();
        let decode_cancel = cancel_token.clone();
        let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
        let (resume_sender, resume_receiver) = mpsc::channel();
        let decode_task = tokio::task::spawn_blocking(move || {
            decode_json_reader::<serde_json::Value, _>(
                PausingReader {
                    inner: Cursor::new(br#"{"answer":"ready"}"#.to_vec()),
                    started: Some(started_sender),
                    resume: resume_receiver,
                },
                decode_cancel,
            )
        });

        tokio::time::timeout(Duration::from_secs(2), started_receiver)
            .await
            .map_err(|_| LlmError::NetworkError("JSON decode did not start".to_string()))?
            .map_err(|_| LlmError::NetworkError("JSON decode observer closed".to_string()))?;
        cancel_token.cancel();
        resume_sender
            .send(())
            .map_err(|error| LlmError::NetworkError(format!("decode resume failed: {error}")))?;
        let result = tokio::time::timeout(Duration::from_secs(2), decode_task)
            .await
            .map_err(|_| LlmError::NetworkError("JSON decode cancellation timed out".to_string()))?
            .map_err(|error| LlmError::InvalidResponse(format!("decode task failed: {error}")))?;
        assert_request_cancellation(result)
    }

    #[tokio::test]
    async fn anthropic_nonstream_rejects_invalid_utf8_json() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = vec![0_u8; 4096];
            let _request_bytes = socket.read(&mut request).await?;
            let mut body = br#"{"content":[{"type":"text","text":""#.to_vec();
            body.push(0xff);
            body.extend_from_slice(br#""}],"stop_reason":"end_turn"}"#);
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(headers.as_bytes()).await?;
            socket.write_all(&body).await?;
            socket.flush().await
        });

        let client =
            AnthropicClient::with_base_url(format!("http://{address}"), "test-key", "test-model");
        let result = client
            .chat(ChatRequest {
                messages: vec![Message::user("hello".to_string())],
                ..Default::default()
            })
            .await;
        assert!(matches!(
            result,
            Err(echo_core::error::ReactError::Llm(error))
                if matches!(error.as_ref(), LlmError::InvalidResponse(_))
        ));
        server
            .await
            .map_err(|error| LlmError::NetworkError(format!("server task failed: {error}")))??;
        Ok(())
    }
}
