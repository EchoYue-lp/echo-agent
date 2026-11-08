use echo_core::error::{LlmError, Result};
use echo_core::llm::LlmTimeouts;
use echo_core::llm::types::ChatCompletionChunk;
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
use tracing::info;

/// 同一SSE边界只运输结构化provider失败；不从message或type/code推自动重试许可。
pub(crate) fn reported_provider_error(provider: &str, error: &serde_json::Value) -> LlmError {
    let Some(object) = error.as_object() else {
        return LlmError::InvalidResponse(format!(
            "{provider} SSE error was not an object: {error}"
        ));
    };
    let Some(message) = object.get("message").and_then(serde_json::Value::as_str) else {
        return LlmError::InvalidResponse(format!(
            "{provider} SSE error omitted string message: {error}"
        ));
    };
    let error_type = match object.get("type") {
        Some(serde_json::Value::String(value)) => Some(value.clone()),
        None | Some(serde_json::Value::Null) => None,
        Some(_) => {
            return LlmError::InvalidResponse(format!(
                "{provider} SSE error type was not a string: {error}"
            ));
        }
    };
    LlmError::ProviderError {
        provider: provider.to_string(),
        error_type,
        code: object.get("code").cloned(),
        message: message.to_string(),
        retryable: false,
    }
}

/// 同一Chat请求的原生外层观察。公开serde负责Frame固定字段/presence校验；
/// 不引用Core私有helper，不从正文、模型别名或canonical fallback重建服务ID。
pub(crate) fn observe_chat_response(
    provider: &str,
    native: &serde_json::Value,
    previous: Option<&echo_core::llm::types::ProviderResponseMetadata>,
) -> Result<echo_core::llm::types::ProviderResponseMetadata> {
    use echo_core::llm::LlmApiProtocol;
    if !native.is_object() {
        return Err(LlmError::InvalidResponse("Chat response must be an object".into()).into());
    }
    if let Some(previous) = previous
        && (previous.provider != provider || previous.protocol != LlmApiProtocol::ChatCompletions)
    {
        return Err(
            LlmError::InvalidResponse("Chat observation request scope changed".into()).into(),
        );
    }
    let native_id = match native.get("id") {
        Some(serde_json::Value::String(id)) if !id.is_empty() => Some(id.clone()),
        Some(serde_json::Value::String(_)) => {
            return Err(
                LlmError::InvalidResponse("Chat native response id was empty".into()).into(),
            );
        }
        None | Some(serde_json::Value::Null) => None,
        Some(_) => {
            return Err(
                LlmError::InvalidResponse("Chat response id must be a string".into()).into(),
            );
        }
    };
    if let (Some(previous), Some(id)) = (
        previous.and_then(|p| p.request_id.as_ref()),
        native_id.as_ref(),
    ) && previous != id
    {
        return Err(LlmError::InvalidResponse(
            "Chat response id changed within one request".into(),
        )
        .into());
    }
    let mut record = match previous {
        Some(previous) => serde_json::to_value(previous),
        None => Ok(serde_json::json!({"protocol":"chat_completions","provider":provider})),
    }
    .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    let fields = record.as_object_mut().ok_or_else(|| {
        LlmError::InvalidResponse("Chat observation record is not an object".into())
    })?;
    if let Some(id) = native_id {
        fields.insert("request_id".into(), serde_json::Value::String(id));
    }
    // 原件显式null必须insert；缺key保留本请求先前值。usage是替换快照，绝不相加。
    for name in ["model", "usage"] {
        if let Some(value) = native.get(name) {
            fields.insert(name.into(), value.clone());
        }
    }
    if let Some(choice) = native
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .and_then(|choices| choices.first())
        && let Some(value) = choice.get("finish_reason")
    {
        fields.insert("stop_reason".into(), value.clone());
    }
    serde_json::from_value(record).map_err(|error| {
        LlmError::InvalidResponse(format!("invalid native Chat observation: {error}")).into()
    })
}

/// 在typed Message/Delta serde之前移除不属于Chat wire的Frame观察，避免连
/// malformed的伪造同名字段都被当作合法Core输入。reasoning_content不在剥除名单。
pub(crate) fn strip_chat_observation_fields(native: &mut serde_json::Value) {
    let Some(root) = native.as_object_mut() else {
        return;
    };
    for name in [
        "provider_response",
        "execution_status",
        "approved_assistant_blocks",
        "assistant_blocks",
    ] {
        root.remove(name);
    }
    for choice in root
        .get_mut("choices")
        .and_then(serde_json::Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        for name in ["message", "delta"] {
            if let Some(message) = choice
                .get_mut(name)
                .and_then(serde_json::Value::as_object_mut)
            {
                for field in [
                    "provider_response",
                    "execution_status",
                    "approved_assistant_blocks",
                    "assistant_blocks",
                    "reasoning_blocks",
                    "tool_result_is_error",
                ] {
                    message.remove(field);
                }
                for call in message
                    .get_mut("tool_calls")
                    .and_then(serde_json::Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    if let Some(call) = call.as_object_mut() {
                        call.remove("provider_source");
                    }
                }
            }
        }
    }
}

/// D2只搬运已正确解码的部分消息；批准和执行状态永不继承。
pub(crate) fn incomplete_chat_response(mut partial: echo_core::llm::types::Message) -> LlmError {
    partial.execution_status = None;
    partial.approved_assistant_blocks = None;
    LlmError::IncompleteResponse {
        partial: Box::new(partial),
    }
}

fn chat_partial_message(
    accumulated: &echo_core::llm::types::Message,
    text: &str,
    calls: &std::collections::BTreeMap<u32, echo_core::llm::types::ToolCall>,
) -> echo_core::llm::types::Message {
    let mut partial = accumulated.clone();
    partial.content = echo_core::llm::types::MessageContent::Text(text.to_string());
    partial.tool_calls = (!calls.is_empty()).then(|| {
        calls
            .iter()
            .map(|(index, call)| {
                let mut call = call.clone();
                // 原生ID可以在usage-only中后到；失败构造时从原map按真实index完整回填，
                // 不要求后面再有tool delta，也不使用canonical fallback冒服务ID。
                let request_id = accumulated
                    .provider_response
                    .as_ref()
                    .and_then(|record| record.request_id.as_deref())
                    .unwrap_or("");
                // 仅D2 partial用空坐标表示未知原生ID；同消息观察仍为None。
                // 此局部校验／失败副本不作为成功回放或跨请求owner。
                call.provider_source = Some(super::native_message::output_source(
                    echo_core::llm::LlmApiProtocol::ChatCompletions,
                    request_id,
                    &call.id,
                    *index,
                    None,
                    "tool_calls",
                    None,
                ));
                call
            })
            .collect()
    });
    partial
}

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

// 极值配置在本地同样可能让Instant加法panic；复用原timeout权威，无法表示时显式报配置错误。
fn checked_stream_deadline(
    started_at: tokio::time::Instant,
    duration: Duration,
    kind: &str,
) -> Result<tokio::time::Instant> {
    started_at.checked_add(duration).ok_or_else(|| {
        echo_core::error::ConfigError::ConfigFileError(format!(
            "LLM stream {kind} timeout cannot be represented as an Instant deadline"
        ))
        .into()
    })
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
/// 复用caller已构造的同一原生请求，保留原URL/header/JSON而不另建请求配置载体。
///
/// `cancel_token` enables aborting the stream: the cancellation signal is checked
/// between each SSE chunk, and iteration stops immediately once cancelled.
/// A choice finish reason and usage are withheld until the final `[DONE]` marker.
#[tracing::instrument(skip(request, cancel_token), fields(model = %model))]
pub(crate) async fn stream_post_json(
    provider: String,
    request: RequestBuilder,
    model: String,
    timeouts: LlmTimeouts,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> Result<impl Stream<Item = Result<ChatCompletionChunk>>> {
    let raw_stream = stream_json_sse(request, model, timeouts, cancel_token).await?;
    Ok(chat_chunks_from_json_sse(provider, raw_stream))
}

// 仍由同一reducer解释Chat语义；私有接缝允许测试注入真实decoder输出而不依赖TCP分帧。
fn chat_chunks_from_json_sse<S>(
    provider: String,
    raw_stream: S,
) -> impl Stream<Item = Result<ChatCompletionChunk>>
where
    S: Stream<Item = Result<JsonSseEvent>> + Send + 'static,
{
    async_stream::try_stream! {
        let mut terminal: Option<ChatCompletionChunk> = None;
        let mut stream_usage = None;
        let mut accumulated = echo_core::llm::types::Message::assistant(String::new());
        let mut text = String::new();
        let mut calls: std::collections::BTreeMap<u32, echo_core::llm::types::ToolCall> = std::collections::BTreeMap::new();
        let mut saw_choice = false;
        futures::pin_mut!(raw_stream);
        while let Some(event) = raw_stream.next().await {
            // 真实HTTP/网络/取消/坏JSON错误保原首因，不套D2。
            match event? {
                JsonSseEvent::Done => {
                    let Some(mut completed) = terminal else {
                        if saw_choice || accumulated.provider_response.is_some() { Err(incomplete_chat_response(chat_partial_message(&accumulated,&text,&calls)))?; }
                        Err(LlmError::InvalidResponse("Chat Completions [DONE] before any finish reason".into()))?;
                        return;
                    };
                    let partial = chat_partial_message(&accumulated,&text,&calls);
                    super::native_message::validate_completion(&partial,completed.choices.first().and_then(|choice|choice.finish_reason.as_deref()))?;
                    // Chat成功必须有真实原生响应身份；未知坐标仅随D2失败返回，
                    // 不能靠空值或生成的fallback取得成功资格。
                    if partial.provider_response.as_ref().and_then(|record| record.request_id.as_deref()).is_none_or(str::is_empty) {
                        Err(incomplete_chat_response(partial))?;
                    }
                    if let Some(choice) = completed.choices.first_mut() {
                        choice.delta.provider_response = accumulated.provider_response.clone();
                        // 仅回填已运输工具的真实坐标，不重发id/name/arguments或创建调用。
                        // 这样晚到ID在成功收口时也不会遗失原稀疏index。
                        if !calls.is_empty() {
                            choice.delta.tool_calls = Some(calls.iter().map(|(index, call)| echo_core::llm::types::DeltaToolCall {
                                provider_source: call.provider_source.clone(), index: *index,
                                id: None, call_type: None, function: None,
                            }).collect());
                        }
                    }
                    completed.usage = stream_usage.or(completed.usage);
                    yield completed;
                    return;
                }
                JsonSseEvent::Data(mut value) => {
                    // HTTP200 SSE供应商错误仍是typed8/allfalse，即使先前已有finish。
                    if let Some(error) = value.get("error") { Err(reported_provider_error(&provider,error))?; }
                    let observation = observe_chat_response(&provider,&value,accumulated.provider_response.as_ref())?;
                    let has_native_observation = value.get("id").is_some() || value.get("model").is_some() || value.get("usage").is_some();
                    strip_chat_observation_fields(&mut value);
                    let mut chunk = serde_json::from_value::<ChatCompletionChunk>(value)
                        .map_err(|error| LlmError::InvalidResponse(format!("invalid Chat Completions SSE event: {error}")))?;
                    if let Some(usage) = chunk.usage.take() { stream_usage = Some(usage); }
                    // 先前terminal后只许usage-only等无choice原件；不靠合成metadata delta绕过检查。
                    if terminal.is_some() && !chunk.choices.is_empty() {
                        Err(LlmError::InvalidResponse("Chat Completions emitted choices after its finish reason".into()))?;
                    }
                    if chunk.choices.is_empty() && !has_native_observation { continue; }
                    accumulated.provider_response = Some(observation);
                    // 真实ID后到时回填原map全部索引，不依赖再收到tool delta。
                    if let Some(request_id) = accumulated.provider_response.as_ref().and_then(|record| record.request_id.as_deref()) {
                        for (index, call) in &mut calls {
                            call.provider_source = Some(super::native_message::output_source(
                                echo_core::llm::LlmApiProtocol::ChatCompletions, request_id, &call.id,
                                *index, None, "tool_calls", None,
                            ));
                        }
                    }
                    if chunk.choices.is_empty() {
                        // metadata-only observation进入同一DeltaMessage；不是成功chunk，
                        // 不造文本/phase/tool或第二公开metadata载体。normalized usage仍hold。
                        chunk.choices.push(echo_core::llm::types::ChunkChoice {
                            index:0, delta:echo_core::llm::types::DeltaMessage {
                                provider_response:accumulated.provider_response.clone(), ..Default::default()
                            }, finish_reason:None,
                        });
                        yield chunk;
                        continue;
                    }
                    saw_choice = true;
                    if let Some(choice) = chunk.choices.first_mut() {
                        choice.delta.provider_response = accumulated.provider_response.clone();
                        if let Some(content) = &choice.delta.content { text.push_str(content); }
                        if let Some(reasoning) = &choice.delta.reasoning_content {
                            accumulated.reasoning_content.get_or_insert_with(String::new).push_str(reasoning);
                        }
                        for delta in choice.delta.tool_calls.as_deref().unwrap_or_default() {
                            let call = calls.entry(delta.index).or_insert_with(|| echo_core::llm::types::ToolCall {
                                provider_source:None,id:String::new(),call_type:"function".into(),
                                function:echo_core::llm::types::FunctionCall{name:String::new(),arguments:String::new()},
                            });
                            if let Some(id) = &delta.id {
                                if !call.id.is_empty() && call.id != *id { Err(LlmError::InvalidResponse("tool call identity changed".into()))?; }
                                call.id = id.clone();
                            }
                            if let Some(function) = &delta.function {
                                if let Some(name) = &function.name { call.function.name.push_str(name); }
                                if let Some(arguments) = &function.arguments { call.function.arguments.push_str(arguments); }
                            }
                            if let Some(request_id) = accumulated.provider_response.as_ref().and_then(|record|record.request_id.as_deref()) {
                                call.provider_source=Some(super::native_message::output_source(echo_core::llm::LlmApiProtocol::ChatCompletions,request_id,&call.id,delta.index,None,"tool_calls",None));
                            }
                        }
                    }
                    let finish = chunk.choices.first().and_then(|choice|choice.finish_reason.as_deref());
                    if let Some(reason) = finish {
                        if !matches!(reason,"stop"|"tool_calls"|"function_call") {
                            Err(incomplete_chat_response(chat_partial_message(&accumulated,&text,&calls)))?;
                        }
                        // 原内容与本次观察先运输；不公开finish/normalized usage，DONE才成功。
                        let mut progress = chunk.clone();
                        if let Some(choice) = progress.choices.first_mut() { choice.finish_reason=None; }
                        progress.usage=None;
                        yield progress;
                        if let Some(choice) = chunk.choices.first_mut() { choice.delta=Default::default(); }
                        terminal=Some(chunk);
                    } else { yield chunk; }
                }
            }
        }
        if saw_choice || accumulated.provider_response.is_some() { Err(incomplete_chat_response(chat_partial_message(&accumulated,&text,&calls)))?; }
        Err(LlmError::InvalidResponse("Chat Completions reached SSE EOF before any decoded response choice".into()))?;
    }
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
    let started_at = tokio::time::Instant::now();
    let deadlines: Result<_> = (|| {
        let overall_deadline = overall_timeout
            .map(|duration| checked_stream_deadline(started_at, duration, "overall"))
            .transpose()?;
        let first_chunk_deadline = first_chunk_timeout
            .map(|duration| {
                checked_stream_deadline(started_at, duration, "first chunk")
                    .map(|deadline| (duration, deadline))
            })
            .transpose()?;
        // 首次I/O前先拒绝不可表示的idle配置；每次实际等待仍按原idle边界重建。
        if let Some(duration) = idle_timeout {
            checked_stream_deadline(started_at, duration, "idle")?;
        }
        Ok((overall_deadline, first_chunk_deadline))
    })();
    // 保留原biased取消首因；已取消请求不能被配置验证或已就绪transport错误覆盖。
    if cancel_token
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Err(LlmError::NetworkError("LLM stream cancelled".to_string()).into());
    }
    let (overall_deadline, first_chunk_deadline) = deadlines?;
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
    let (byte_stream, first_bytes) = tokio::select! {
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
            match first_chunk_deadline {
                Some((duration, deadline)) => tokio::time::timeout_at(deadline, &mut start_stream)
                    .await
                    .map_err(|_| timeout_error("first chunk", duration)),
                None => Ok((&mut start_stream).await),
            }
        } => result?,
    }?;

    Ok(decode_json_sse_bytes(
        byte_stream,
        first_bytes,
        idle_timeout,
        overall_timeout,
        overall_deadline,
        cancel_token,
    ))
}

// 同一生产decoder和原deadline；只把已取得的bytes输入做成私有可注入边界。
fn decode_json_sse_bytes<S, B, E>(
    byte_stream: S,
    first_bytes: B,
    idle_timeout: Option<Duration>,
    overall_timeout: Option<Duration>,
    overall_deadline: Option<tokio::time::Instant>,
    cancel_token: Option<CancellationToken>,
) -> impl Stream<Item = Result<JsonSseEvent>>
where
    S: Stream<Item = std::result::Result<B, E>> + Send + 'static,
    B: AsRef<[u8]> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    async_stream::try_stream! {
        futures::pin_mut!(byte_stream);
        let mut decoder = SseDecoder::new();
        decoder.push(first_bytes.as_ref())?;
        while let Some(event) = decoder.next_event() {
            // yield恢复后仍检查同一请求，缓存中的finish/DONE不能越过取消边界。
            if cancel_token.as_ref().is_some_and(CancellationToken::is_cancelled) {
                Err(LlmError::NetworkError("LLM stream cancelled".to_string()))?;
            }
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
                } => Err::<_, echo_core::error::ReactError>(
                    LlmError::NetworkError("LLM stream cancelled".to_string()).into()
                ),
                duration = wait_for_deadline(overall_timeout, overall_deadline) => {
                    Err(timeout_error("overall", duration).into())
                },
                result = async {
                    match idle_timeout {
                        Some(duration) => {
                            let deadline = checked_stream_deadline(
                                tokio::time::Instant::now(), duration, "idle"
                            )?;
                            tokio::time::timeout_at(deadline, next_bytes)
                                .await
                                .map_err(|_| echo_core::error::ReactError::from(timeout_error("idle", duration)))
                        },
                        None => Ok(next_bytes.await),
                    }
                } => result,
            }?;
            let Some(bytes) = bytes else {
                break;
            };
            let bytes = bytes.map_err(|error| LlmError::NetworkError(error.to_string()))?;
            decoder.push(bytes.as_ref())?;
            while let Some(event) = decoder.next_event() {
                // 与首批bytes相同：交付每个缓存事件前复核，已yield的事件不撤回。
                if cancel_token.as_ref().is_some_and(CancellationToken::is_cancelled) {
                    Err(LlmError::NetworkError("LLM stream cancelled".to_string()))?;
                }
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
    }
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

    // 以下输入进入原decoder和原Chat reducer，不开socket，不假定TCP write对应bytes chunk。
    fn source8_memory_sse(
        first: Vec<u8>,
        later: Vec<Vec<u8>>,
        cancellation: Option<CancellationToken>,
    ) -> impl Stream<Item = Result<JsonSseEvent>> {
        let source = futures::stream::iter(later.into_iter().map(Ok::<_, std::io::Error>));
        decode_json_sse_bytes(source, first, None, None, None, cancellation)
    }

    fn source8_assert_cancelled<T>(result: Result<T>) -> Result<()> {
        match result {
            Err(echo_core::error::ReactError::Llm(error)) if matches!(error.as_ref(), LlmError::NetworkError(message) if message == "LLM stream cancelled") => {
                Ok(())
            }
            Err(error) => Err(error),
            Ok(_) => Err(LlmError::InvalidResponse(
                "cached SSE event bypassed cancellation".into(),
            )
            .into()),
        }
    }

    fn source8_chat_packets() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let first = serde_json::json!({
            "id":"source8-native", "model":"source8-model", "choices":[{
                "index":0,"delta":{"role":"assistant","content":"首块🙂"},"finish_reason":null
            }]
        });
        let finish = serde_json::json!({
            "id":"source8-native", "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}
        });
        (
            format!("data: {first}\n\n").into_bytes(),
            format!("data: {finish}\n\n").into_bytes(),
            b"data: [DONE]\n\n".to_vec(),
        )
    }

    fn source8_byte_framings(first: &[u8], tail: &[u8]) -> Vec<(Vec<u8>, Vec<Vec<u8>>)> {
        let packed = [first, tail].concat();
        vec![
            (packed.clone(), vec![]),
            (
                b": first bytes contain no data event\n\n".to_vec(),
                vec![packed],
            ),
            (first.to_vec(), vec![tail.to_vec()]),
            (
                b": first bytes contain no data event\n\n".to_vec(),
                vec![first.to_vec(), tail.to_vec()],
            ),
        ]
    }

    #[tokio::test]
    async fn source8_sse_cancel_before_first_poll_rejects_buffered_event() -> Result<()> {
        let (first, finish, done) = source8_chat_packets();
        let cancellation = CancellationToken::new();
        let raw = source8_memory_sse(
            [first, finish, done].concat(),
            vec![],
            Some(cancellation.clone()),
        );
        futures::pin_mut!(raw);
        cancellation.cancel();
        source8_assert_cancelled(raw.next().await.ok_or(LlmError::EmptyResponse)?)?;
        assert!(raw.next().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn source8_sse_cancel_after_delta_is_independent_of_byte_framing() -> Result<()> {
        let (first, finish, done) = source8_chat_packets();
        for (head, tail) in source8_byte_framings(&first, &[finish, done].concat()) {
            let cancellation = CancellationToken::new();
            let raw = source8_memory_sse(head, tail, Some(cancellation.clone()));
            futures::pin_mut!(raw);
            let event = raw.next().await.ok_or(LlmError::EmptyResponse)??;
            let JsonSseEvent::Data(value) = event else {
                return Err(
                    LlmError::InvalidResponse("test expected original first delta".into()).into(),
                );
            };
            assert_eq!(
                value
                    .pointer("/choices/0/delta/content")
                    .and_then(serde_json::Value::as_str),
                Some("首块🙂")
            );
            cancellation.cancel();
            source8_assert_cancelled(raw.next().await.ok_or(LlmError::EmptyResponse)?)?;
            assert!(raw.next().await.is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn source8_sse_utf8_byte_splits_preserve_data_and_cancel_boundary() -> Result<()> {
        let (first, finish, done) = source8_chat_packets();
        let wire = [first, finish, done].concat();
        for cancelled in [false, true] {
            let mut parts = wire.chunks(1).map(<[u8]>::to_vec);
            let head = parts.next().ok_or(LlmError::EmptyResponse)?;
            let cancellation = CancellationToken::new();
            let raw = source8_memory_sse(head, parts.collect(), Some(cancellation.clone()));
            futures::pin_mut!(raw);
            let JsonSseEvent::Data(value) = raw.next().await.ok_or(LlmError::EmptyResponse)??
            else {
                return Err(
                    LlmError::InvalidResponse("test expected original UTF-8 delta".into()).into(),
                );
            };
            assert_eq!(
                value
                    .pointer("/choices/0/delta/content")
                    .and_then(serde_json::Value::as_str),
                Some("首块🙂")
            );
            if cancelled {
                cancellation.cancel();
                source8_assert_cancelled(raw.next().await.ok_or(LlmError::EmptyResponse)?)?;
            } else {
                let JsonSseEvent::Data(finish) =
                    raw.next().await.ok_or(LlmError::EmptyResponse)??
                else {
                    return Err(
                        LlmError::InvalidResponse("test expected original finish".into()).into(),
                    );
                };
                assert_eq!(
                    finish
                        .pointer("/choices/0/finish_reason")
                        .and_then(serde_json::Value::as_str),
                    Some("stop")
                );
                assert!(matches!(
                    raw.next().await.ok_or(LlmError::EmptyResponse)??,
                    JsonSseEvent::Done
                ));
            }
            assert!(raw.next().await.is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn source8_chat_cancel_after_delta_withholds_cached_finish_and_usage() -> Result<()> {
        let (first, finish, done) = source8_chat_packets();
        for (head, tail) in source8_byte_framings(&first, &[finish, done].concat()) {
            let cancellation = CancellationToken::new();
            let raw = source8_memory_sse(head, tail, Some(cancellation.clone()));
            let chunks = chat_chunks_from_json_sse("source8-provider".into(), raw);
            futures::pin_mut!(chunks);
            let first = chunks.next().await.ok_or(LlmError::EmptyResponse)??;
            let choice = first.choices.first().ok_or(LlmError::EmptyResponse)?;
            assert_eq!(choice.delta.content.as_deref(), Some("首块🙂"));
            assert!(choice.finish_reason.is_none() && first.usage.is_none());
            assert_eq!(
                choice
                    .delta
                    .provider_response
                    .as_ref()
                    .and_then(|value| value.request_id.as_deref()),
                Some("source8-native")
            );
            cancellation.cancel();
            source8_assert_cancelled(chunks.next().await.ok_or(LlmError::EmptyResponse)?)?;
            assert!(chunks.next().await.is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn source8_chat_cancel_after_held_finish_does_not_publish_pending_done() -> Result<()> {
        let (first, finish, done) = source8_chat_packets();
        for (head, tail) in source8_byte_framings(&[first, finish].concat(), &done) {
            let cancellation = CancellationToken::new();
            let chunks = chat_chunks_from_json_sse(
                "source8-provider".into(),
                source8_memory_sse(head, tail, Some(cancellation.clone())),
            );
            futures::pin_mut!(chunks);
            for _ in 0..2 {
                let progress = chunks.next().await.ok_or(LlmError::EmptyResponse)??;
                assert!(
                    progress
                        .choices
                        .iter()
                        .all(|choice| choice.finish_reason.is_none())
                );
                assert!(progress.usage.is_none());
            }
            cancellation.cancel();
            source8_assert_cancelled(chunks.next().await.ok_or(LlmError::EmptyResponse)?)?;
            assert!(chunks.next().await.is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn source8_sse_done_already_yielded_remains_finished_after_cancel() -> Result<()> {
        let cancellation = CancellationToken::new();
        let raw = source8_memory_sse(
            b"data: [DONE]\n\n".to_vec(),
            vec![],
            Some(cancellation.clone()),
        );
        futures::pin_mut!(raw);
        assert!(matches!(
            raw.next().await.ok_or(LlmError::EmptyResponse)??,
            JsonSseEvent::Done
        ));
        cancellation.cancel();
        assert!(raw.next().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn source8_chat_published_terminal_is_not_replaced_by_later_cancel() -> Result<()> {
        let (first, finish, done) = source8_chat_packets();
        for (head, tail) in source8_byte_framings(&first, &[finish, done].concat()) {
            let cancellation = CancellationToken::new();
            let chunks = chat_chunks_from_json_sse(
                "source8-provider".into(),
                source8_memory_sse(head, tail, Some(cancellation.clone())),
            );
            futures::pin_mut!(chunks);
            for _ in 0..2 {
                let progress = chunks.next().await.ok_or(LlmError::EmptyResponse)??;
                assert!(
                    progress
                        .choices
                        .iter()
                        .all(|choice| choice.finish_reason.is_none())
                );
                assert!(progress.usage.is_none());
            }
            let terminal = chunks.next().await.ok_or(LlmError::EmptyResponse)??;
            assert_eq!(
                terminal
                    .choices
                    .first()
                    .and_then(|choice| choice.finish_reason.as_deref()),
                Some("stop")
            );
            assert_eq!(
                terminal.usage.as_ref().and_then(|usage| usage.total_tokens),
                Some(3)
            );
            assert_eq!(
                terminal
                    .choices
                    .first()
                    .and_then(|choice| choice.delta.provider_response.as_ref())
                    .and_then(|value| value.request_id.as_deref()),
                Some("source8-native")
            );
            cancellation.cancel();
            assert!(chunks.next().await.is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn source8_sse_raw_native_values_are_unchanged_before_cancellation() -> Result<()> {
        let values = vec![
            serde_json::json!({"id":"chat-native","choices":[{"index":0,"delta":{"content":"中文🙂","reasoning_content":"原推理"}}]}),
            serde_json::json!({"type":"content_block_delta","index":4,"delta":{"type":"signature_delta","signature":"原签名"}}),
            serde_json::json!({"type":"response.output_item.done","output_index":2,"item":{"id":"reasoning-native","type":"reasoning","encrypted_content":"opaque-original","summary":[]}}),
        ];
        for expected in values {
            let wire = format!("data: {expected}\n\ndata: [DONE]\n\n").into_bytes();
            let cancellation = CancellationToken::new();
            let raw = source8_memory_sse(wire, vec![], Some(cancellation.clone()));
            futures::pin_mut!(raw);
            let JsonSseEvent::Data(actual) = raw.next().await.ok_or(LlmError::EmptyResponse)??
            else {
                return Err(LlmError::InvalidResponse(
                    "test expected unchanged native JSON".into(),
                )
                .into());
            };
            assert_eq!(actual, expected);
            cancellation.cancel();
            source8_assert_cancelled(raw.next().await.ok_or(LlmError::EmptyResponse)?)?;
        }
        // 原生JSON字段保留，不冒三种真实adapter/HTTP/签名回放已在此运行。
        Ok(())
    }

    #[tokio::test]
    async fn source8_chat_provider_error_already_observed_keeps_original_cause() -> Result<()> {
        let cancellation = CancellationToken::new();
        let raw = source8_memory_sse(b"data: {\"error\":{\"type\":\"invalid_request_error\",\"code\":37,\"message\":\"original cause\"}}\n\ndata: [DONE]\n\n".to_vec(), vec![], Some(cancellation.clone()));
        let chunks = chat_chunks_from_json_sse("source8-provider".into(), raw);
        futures::pin_mut!(chunks);
        let error = chunks.next().await.ok_or(LlmError::EmptyResponse)?;
        assert!(
            matches!(error, Err(echo_core::error::ReactError::Llm(error))
            if matches!(error.as_ref(),LlmError::ProviderError{code:Some(code),retryable:false,..} if code==&serde_json::json!(37)))
        );
        cancellation.cancel();
        assert!(chunks.next().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn source8_openai_component_cancel_after_delta_never_closes_cached_block() -> Result<()> {
        let (first, finish, done) = source8_chat_packets();
        for (head, tail) in source8_byte_framings(&first, &[finish, done].concat()) {
            let cancellation = CancellationToken::new();
            let raw = source8_memory_sse(head, tail, Some(cancellation.clone()));
            let chunks = super::super::openai::adapt_openai_stream(
                chat_chunks_from_json_sse("source8-provider".into(), raw),
                "source8-provider".into(),
            );
            futures::pin_mut!(chunks);
            let first = chunks.next().await.ok_or(LlmError::EmptyResponse)??;
            assert_eq!(first.delta.content.as_deref(), Some("首块🙂"));
            assert!(first.finish_reason.is_none() && first.usage.is_none());
            let block = first
                .delta
                .assistant_blocks
                .as_ref()
                .and_then(|blocks| blocks.first())
                .ok_or(LlmError::EmptyResponse)?;
            assert_eq!(block.request_id, "source8-native");
            assert_eq!(block.block_id, "source8-native:assistant");
            assert!(!block.complete);
            cancellation.cancel();
            source8_assert_cancelled(chunks.next().await.ok_or(LlmError::EmptyResponse)?)?;
            assert!(chunks.next().await.is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn source8_openai_component_cancel_after_held_finish_keeps_original_observation()
    -> Result<()> {
        let (first, finish, done) = source8_chat_packets();
        for (head, tail) in source8_byte_framings(&[first, finish].concat(), &done) {
            let cancellation = CancellationToken::new();
            let chunks = super::super::openai::adapt_openai_stream(
                chat_chunks_from_json_sse(
                    "source8-provider".into(),
                    source8_memory_sse(head, tail, Some(cancellation.clone())),
                ),
                "source8-provider".into(),
            );
            futures::pin_mut!(chunks);
            for _ in 0..2 {
                let observed = chunks.next().await.ok_or(LlmError::EmptyResponse)??;
                assert!(observed.finish_reason.is_none() && observed.usage.is_none());
                assert_eq!(
                    observed
                        .delta
                        .provider_response
                        .as_ref()
                        .and_then(|value| value.request_id.as_deref()),
                    Some("source8-native")
                );
                assert!(
                    observed
                        .delta
                        .assistant_blocks
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .all(|block| !block.complete)
                );
            }
            cancellation.cancel();
            source8_assert_cancelled(chunks.next().await.ok_or(LlmError::EmptyResponse)?)?;
            assert!(chunks.next().await.is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn source8_openai_component_success_preserves_native_identity_phase_and_usage()
    -> Result<()> {
        let (first, finish, done) = source8_chat_packets();
        for (head, tail) in source8_byte_framings(&first, &[finish, done].concat()) {
            let chunks = super::super::openai::adapt_openai_stream(
                chat_chunks_from_json_sse(
                    "source8-provider".into(),
                    source8_memory_sse(head, tail, None),
                ),
                "source8-provider".into(),
            );
            let values = chunks
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>>>()?;
            assert_eq!(values.len(), 3);
            let first = values.first().ok_or(LlmError::EmptyResponse)?;
            let first_block = first
                .delta
                .assistant_blocks
                .as_ref()
                .and_then(|blocks| blocks.first())
                .ok_or(LlmError::EmptyResponse)?;
            assert_eq!(first_block.phase, echo_core::agent::AssistantPhase::Unknown);
            assert_eq!(
                first_block
                    .provider_source
                    .as_ref()
                    .map(|source| source.request_id.as_str()),
                Some("source8-native")
            );
            let terminal = values.last().ok_or(LlmError::EmptyResponse)?;
            assert_eq!(terminal.finish_reason.as_deref(), Some("stop"));
            assert_eq!(
                terminal.usage.as_ref().and_then(|usage| usage.total_tokens),
                Some(3)
            );
            let closed = terminal
                .delta
                .assistant_blocks
                .as_ref()
                .and_then(|blocks| blocks.first())
                .ok_or(LlmError::EmptyResponse)?;
            assert!(closed.complete && closed.delta.is_empty());
            assert_eq!(closed.block_id, first_block.block_id);
            assert_eq!(closed.request_id, first_block.request_id);
            assert_eq!(closed.phase, echo_core::agent::AssistantPhase::Unknown);
        }
        Ok(())
    }

    #[tokio::test]
    async fn source8_openai_component_published_terminal_is_not_replaced_by_cancel() -> Result<()> {
        let (first, finish, done) = source8_chat_packets();
        for (head, tail) in source8_byte_framings(&first, &[finish, done].concat()) {
            let cancellation = CancellationToken::new();
            let chunks = super::super::openai::adapt_openai_stream(
                chat_chunks_from_json_sse(
                    "source8-provider".into(),
                    source8_memory_sse(head, tail, Some(cancellation.clone())),
                ),
                "source8-provider".into(),
            );
            futures::pin_mut!(chunks);
            for _ in 0..2 {
                let observed = chunks.next().await.ok_or(LlmError::EmptyResponse)??;
                assert!(observed.finish_reason.is_none());
            }
            let terminal = chunks.next().await.ok_or(LlmError::EmptyResponse)??;
            assert_eq!(terminal.finish_reason.as_deref(), Some("stop"));
            assert_eq!(
                terminal.usage.as_ref().and_then(|usage| usage.total_tokens),
                Some(3)
            );
            cancellation.cancel();
            assert!(chunks.next().await.is_none());
        }
        Ok(())
    }

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

    async fn collect_provider_json(
        provider: TestProvider,
        body: serde_json::Value,
    ) -> Result<echo_core::llm::ChatResponse> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let body = body.to_string();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut buffer = vec![0u8; 8192];
            let _read = socket.read(&mut buffer).await?;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await?;
            socket.write_all(body.as_bytes()).await?;
            socket.flush().await
        });
        let client = test_provider_client(provider, format!("http://{address}"))?;
        let result = client
            .chat(ChatRequest::new(vec![Message::user("hello".into())]))
            .await;
        server
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))??;
        result
    }

    fn native_text(
        blocks: impl IntoIterator<Item = echo_core::agent::AssistantMessageBlock>,
    ) -> std::collections::BTreeMap<String, (echo_core::agent::AssistantPhase, String)> {
        let mut values = std::collections::BTreeMap::new();
        for block in blocks {
            let value = values
                .entry(block.block_id)
                .or_insert_with(|| (block.phase, String::new()));
            value.1.push_str(&block.delta);
        }
        values
    }

    #[tokio::test]
    async fn provider_native_responses_stream_nonstream_preserve_multiple_messages_and_tools()
    -> Result<()> {
        use serde_json::{Value, json};
        let mut full: Value = serde_json::from_str(include_str!(
            "../../fixtures/providers/responses/multi-output.json"
        ))
        .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
        full.pointer_mut("/output/0/content")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| LlmError::InvalidResponse("fixture content missing".into()))?
            .insert(
                0,
                serde_json::json!({"type":"output_text","text":"","annotations":[]}),
            );
        let response = collect_provider_json(TestProvider::Responses, full.clone()).await?;
        let mut events =
            vec![json!({"type":"response.created","response":{"id":"resp_fixture_multi"}})];
        for (index, item) in full
            .get("output")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| LlmError::InvalidResponse("fixture id missing".into()))?;
            events.push(
                json!({"type":"response.output_item.added","output_index":index,"item":item}),
            );
            if item.get("type").and_then(Value::as_str) == Some("message") {
                for (content_index, part) in item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
                    events.push(json!({"type":"response.output_text.delta","output_index":index,"content_index":content_index,"item_id":id,"delta":text}));
                    events.push(json!({"type":"response.output_text.done","output_index":index,"content_index":content_index,"item_id":id,"text":text}));
                }
            } else if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                for (summary_index, part) in item
                    .get("summary")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
                    events.push(json!({"type":"response.reasoning_summary_text.delta","output_index":index,"summary_index":summary_index,"item_id":id,"delta":text}));
                    events.push(json!({"type":"response.reasoning_summary_text.done","output_index":index,"summary_index":summary_index,"item_id":id,"text":text}));
                }
            }
            events
                .push(json!({"type":"response.output_item.done","output_index":index,"item":item}));
        }
        events.push(json!({"type":"response.completed","response":full}));
        let encoded = events.iter().map(Value::to_string).collect::<Vec<_>>();
        let borrowed = encoded.iter().map(String::as_str).collect::<Vec<_>>();
        let chunks = collect_provider_sse(TestProvider::Responses, &borrowed)
            .await?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(
            chunks.iter().filter(|c| c.finish_reason.is_some()).count(),
            1
        );
        assert_eq!(
            chunks.last().and_then(|c| c.finish_reason.as_deref()),
            Some("tool_calls")
        );
        let stream_blocks = chunks
            .iter()
            .flat_map(|c| c.delta.assistant_blocks.clone().unwrap_or_default())
            .collect::<Vec<_>>();
        assert!(stream_blocks.windows(2).all(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_none_or(|(a, b)| a.sequence < b.sequence)
        }));
        assert_eq!(
            native_text(stream_blocks),
            native_text(
                response
                    .message
                    .assistant_blocks
                    .clone()
                    .unwrap_or_default()
            )
        );
        assert!(
            response
                .message
                .assistant_blocks
                .as_ref()
                .is_some_and(|blocks| blocks
                    .iter()
                    .any(|b| b.phase == echo_core::agent::AssistantPhase::Unknown
                        && b.delta == "工具结果已读取。"))
        );
        let tools = chunks
            .iter()
            .flat_map(|c| c.delta.tool_calls.as_deref().unwrap_or_default())
            .filter_map(|c| c.id.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(tools, vec!["call_1", "call_2"]);
        Ok(())
    }

    #[tokio::test]
    async fn provider_native_nonstream_rejects_empty_reasoning_only_and_truncated() -> Result<()> {
        use serde_json::json;
        let cases = [
            (
                TestProvider::OpenAi,
                json!({"id":"chat_empty","choices":[{"message":{"role":"assistant","content":""},"finish_reason":"stop"}]}),
            ),
            (
                TestProvider::OpenAi,
                json!({"id":"chat_cut","choices":[{"message":{"role":"assistant","content":"partial"},"finish_reason":"length"}]}),
            ),
            (
                TestProvider::Responses,
                json!({"id":"resp_empty","status":"completed","output":[]}),
            ),
            (
                TestProvider::Responses,
                json!({"id":"resp_reasoning","status":"completed","output":[{"type":"reasoning","id":"rs_1","encrypted_content":"opaque","summary":[]}]}),
            ),
            (
                TestProvider::Anthropic,
                json!({"id":"msg_empty","content":[],"stop_reason":"end_turn"}),
            ),
            (
                TestProvider::Anthropic,
                json!({"id":"msg_cut","content":[{"type":"text","text":"partial"}],"stop_reason":"max_tokens"}),
            ),
            (
                TestProvider::Anthropic,
                json!({"id":"msg_unknown","content":[{"type":"text","text":"done"}],"stop_reason":"future_reason"}),
            ),
        ];
        for (provider, body) in cases {
            assert!(
                collect_provider_json(provider, body).await.is_err(),
                "{} accepted invalid completion",
                provider.name()
            );
        }
        Ok(())
    }

    async fn provider_http_history_roundtrip(
        provider: TestProvider,
        response: serde_json::Value,
        from_stream: Option<Message>,
    ) -> Result<(Message, Vec<serde_json::Value>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let body = response.to_string();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await?;
                let mut bytes = Vec::new();
                loop {
                    let count = socket.read_buf(&mut bytes).await?;
                    if count == 0 {
                        return Err::<Vec<serde_json::Value>, echo_core::error::ReactError>(
                            LlmError::InvalidResponse("HTTP fixture request truncated".into())
                                .into(),
                        );
                    }
                    let text = match std::str::from_utf8(&bytes) {
                        Ok(text) => text,
                        Err(error) if error.error_len().is_none() => continue,
                        Err(error) => {
                            return Err(LlmError::InvalidResponse(error.to_string()).into());
                        }
                    };
                    if let Some((headers, payload)) = text.split_once("\r\n\r\n") {
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                    .map(|(_, value)| value.trim().parse::<usize>())
                            })
                            .transpose()
                            .map_err(|error| LlmError::InvalidResponse(error.to_string()))?
                            .ok_or_else(|| {
                                LlmError::InvalidResponse(
                                    "HTTP fixture omitted Content-Length".into(),
                                )
                            })?;
                        if payload.len() >= length {
                            requests.push(
                                serde_json::from_str(payload).map_err(|error| {
                                    LlmError::InvalidResponse(error.to_string())
                                })?,
                            );
                            break;
                        }
                    }
                }
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await?;
                socket.write_all(body.as_bytes()).await?;
                socket.flush().await?;
            }
            Ok(requests)
        });
        let client = test_provider_client(provider, format!("http://{address}"))?;
        let result = async {
            let response = client
                .chat(ChatRequest::new(vec![Message::user("request".into())]))
                .await?;
            let mut history = from_stream.unwrap_or(response.message);
            let original = history.clone();
            history.execution_status = Some(echo_core::agent::ExecutionStatus {
                adapter_generated: false,
                execution_id: "runtime-only".into(),
                request_id: Some("runtime-request".into()),
                sequence: 5,
                phase: echo_core::agent::AgentPhase::Thinking,
                status: echo_core::agent::ExecutionStatusKind::Candidate,
                reason: None,
            });
            history.approved_assistant_blocks = history
                .assistant_blocks
                .as_ref()
                .and_then(|blocks| blocks.first())
                .map(|block| {
                    let mut approved = block.clone();
                    approved.block_id = "runtime:assistant:final".into();
                    approved.delta = "APPROVED_RUNTIME_ONLY".into();
                    approved.provider = Some("runtime".into());
                    vec![approved]
                });
            let bytes = serde_json::to_vec(&history)
                .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
            let restored: Message = serde_json::from_slice(&bytes)
                .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
            client
                .chat(ChatRequest::new(vec![
                    Message::user("request".into()),
                    restored,
                ]))
                .await?;
            Ok::<Message, echo_core::error::ReactError>(original)
        }
        .await;
        match result {
            Ok(original) => {
                let requests = server
                    .await
                    .map_err(|error| LlmError::NetworkError(error.to_string()))??;
                Ok((original, requests))
            }
            Err(error) => {
                server.abort();
                let _settled = server.await;
                Err(error)
            }
        }
    }

    fn has_runtime_wire_fields(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(fields) => fields.iter().any(|(key, value)| {
                matches!(
                    key.as_str(),
                    "execution_status"
                        | "approved_assistant_blocks"
                        | "assistant_blocks"
                        | "provider_source"
                ) || has_runtime_wire_fields(value)
            }),
            serde_json::Value::Array(values) => values.iter().any(has_runtime_wire_fields),
            _ => false,
        }
    }

    #[tokio::test]
    async fn provider_native_v4_http_refusal_tool_order_roundtrip() -> Result<()> {
        use serde_json::Value;
        let mut raw: Value = serde_json::from_str(include_str!(
            "../../fixtures/providers/responses/refusal-tool-roundtrip.json"
        ))
        .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
        raw.pointer_mut("/output/0/content")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| LlmError::InvalidResponse("fixture content missing".into()))?
            .insert(
                0,
                serde_json::json!({"type":"output_text","text":"","annotations":[]}),
            );
        let (original, requests) =
            provider_http_history_roundtrip(TestProvider::Responses, raw, None).await?;
        assert!(original.execution_status.is_none());
        assert!(original.approved_assistant_blocks.is_none());
        let request = requests
            .get(1)
            .ok_or_else(|| LlmError::InvalidResponse("second HTTP request missing".into()))?;
        assert!(
            !has_runtime_wire_fields(request),
            "runtime metadata escaped into Responses request"
        );
        let items = request
            .get("input")
            .and_then(Value::as_array)
            .ok_or_else(|| LlmError::InvalidResponse("Responses input missing".into()))?;
        let native = items
            .iter()
            .filter(|item| item.get("role").and_then(Value::as_str) != Some("user"))
            .collect::<Vec<_>>();
        let kinds = native
            .iter()
            .filter_map(|item| item.get("type").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                "message",
                "function_call",
                "reasoning",
                "message",
                "function_call"
            ]
        );
        assert_eq!(
            native
                .first()
                .and_then(|item| item.get("content"))
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(3)
        );
        assert_eq!(
            native
                .first()
                .and_then(|item| item.get("content"))
                .and_then(Value::as_array)
                .and_then(|parts| parts.first())
                .and_then(|part| part.get("text"))
                .and_then(Value::as_str),
            Some("")
        );
        assert_eq!(
            native
                .first()
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str),
            Some("msg_before")
        );
        assert_eq!(
            native
                .first()
                .and_then(|item| item.get("phase"))
                .and_then(Value::as_str),
            Some("commentary")
        );
        assert_eq!(
            native
                .first()
                .and_then(|item| item.get("content"))
                .and_then(Value::as_array)
                .and_then(|parts| parts.get(2))
                .and_then(|part| part.get("type"))
                .and_then(Value::as_str),
            Some("refusal")
        );
        assert_eq!(
            native
                .first()
                .and_then(|item| item.get("content"))
                .and_then(Value::as_array)
                .and_then(|parts| parts.get(2))
                .and_then(|part| part.get("refusal"))
                .and_then(Value::as_str),
            Some("保留拒绝块")
        );
        assert_eq!(
            native
                .get(1)
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str),
            Some("fc_before")
        );
        assert_eq!(
            native
                .get(2)
                .and_then(|item| item.get("encrypted_content"))
                .and_then(Value::as_str),
            Some("opaque-byte-for-byte")
        );
        assert_eq!(
            native
                .get(3)
                .and_then(|item| item.get("phase"))
                .and_then(Value::as_str),
            Some("future_provider_phase")
        );
        assert_eq!(
            native
                .get(4)
                .and_then(|item| item.get("call_id"))
                .and_then(Value::as_str),
            Some("call_after")
        );
        Ok(())
    }

    #[tokio::test]
    async fn provider_native_v4_http_runtime_fields_are_not_wire_or_provider_facts() -> Result<()> {
        use serde_json::json;
        let spoof = json!({"execution_id":"spoof-provider","request_id":"chat_spoof","sequence":99,"phase":"Terminal","status":"settled","reason":null});
        let cases = [
            (
                TestProvider::OpenAi,
                json!({"id":"chat_spoof","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"wire answer","execution_status":spoof,"approved_assistant_blocks":[]}}]}),
            ),
            (
                TestProvider::Anthropic,
                json!({"id":"msg_wire","content":[{"type":"thinking","thinking":"private","signature":"native-sig"},{"type":"text","text":"wire answer"}],"stop_reason":"end_turn","execution_status":spoof}),
            ),
        ];
        for (provider, raw) in cases {
            let (original, requests) = provider_http_history_roundtrip(provider, raw, None).await?;
            assert!(
                original.execution_status.is_none(),
                "{} trusted provider runtime status",
                provider.name()
            );
            assert!(
                original.approved_assistant_blocks.is_none(),
                "{} trusted provider approved content",
                provider.name()
            );
            let request = requests
                .get(1)
                .ok_or_else(|| LlmError::InvalidResponse("second HTTP request missing".into()))?;
            assert!(
                !has_runtime_wire_fields(request),
                "{} serialized runtime fields",
                provider.name()
            );
            assert!(!request.to_string().contains("APPROVED_RUNTIME_ONLY"));
            if matches!(provider, TestProvider::Anthropic) {
                assert!(request.to_string().contains("native-sig"));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn provider_native_v4_http_late_phase_stream_roundtrip() -> Result<()> {
        use serde_json::{Value, json};
        for phase_on_item in [true, false] {
            let full = json!({"id":"resp_late","status":"completed","output":[{"type":"message","id":"msg_late","phase":"final_answer","status":"completed","content":[{"type":"output_text","text":"answer","annotations":[]}]}]});
            let mut events = vec![
                json!({"type":"response.created","response":{"id":"resp_late"}}),
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_late","content":[]}}),
                json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"item_id":"msg_late","delta":"answer"}),
                json!({"type":"response.output_text.done","output_index":0,"content_index":0,"item_id":"msg_late","text":"answer"}),
                json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_late","phase":"final_answer","status":"completed","content":[{"type":"output_text","text":"answer","annotations":[]}]}}),
                json!({"type":"response.completed","response":full}),
            ];
            if !phase_on_item {
                for event in &mut events {
                    if event.get("type").and_then(Value::as_str)
                        == Some("response.output_item.done")
                        && let Some(item) = event.get_mut("item").and_then(Value::as_object_mut)
                    {
                        item.remove("phase");
                    }
                }
            }
            let encoded = events.iter().map(Value::to_string).collect::<Vec<_>>();
            let refs = encoded.iter().map(String::as_str).collect::<Vec<_>>();
            let chunks = collect_provider_sse(TestProvider::Responses, &refs)
                .await?
                .into_iter()
                .collect::<Result<Vec<_>>>()?;
            let blocks = chunks
                .iter()
                .flat_map(|chunk| chunk.delta.assistant_blocks.clone().unwrap_or_default())
                .collect::<Vec<_>>();
            assert!(
                blocks
                    .iter()
                    .filter(|block| block.complete)
                    .all(|block| block.phase == echo_core::agent::AssistantPhase::FinalAnswer)
            );
            assert!(blocks.last().is_some_and(|block| {
                block.complete
                    && block.phase == echo_core::agent::AssistantPhase::FinalAnswer
                    && block
                        .provider_source
                        .as_ref()
                        .and_then(|source| source.phase.as_deref())
                        == Some("final_answer")
            }));
            let mut history = Message::assistant("answer".into());
            history.assistant_blocks = Some(blocks);
            let (_, requests) =
                provider_http_history_roundtrip(TestProvider::Responses, full, Some(history))
                    .await?;
            let message = requests
                .get(1)
                .and_then(|request| request.get("input"))
                .and_then(Value::as_array)
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| item.get("role").and_then(Value::as_str) == Some("assistant"))
                })
                .ok_or_else(|| LlmError::InvalidResponse("native replay message omitted".into()))?;
            assert_eq!(
                message.get("phase").and_then(Value::as_str),
                Some("final_answer")
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn provider_streams_require_semantic_completion_after_partial_output() -> Result<()> {
        use serde_json::{Value, json};
        enum Expected {
            Incomplete { metadata: Option<Value> },
        }
        let cases: &[(TestProvider, &[&str], Expected)] = &[
            (
                TestProvider::OpenAi,
                &[r#"{"choices":[{"delta":{"content":"partial"},"index":0}]}"#],
                Expected::Incomplete {
                    metadata: Some(json!({"protocol":"chat_completions","provider":"openai"})),
                },
            ),
            (
                TestProvider::OpenAi,
                &[
                    r#"{"choices":[{"delta":{"content":"partial"},"index":0}]}"#,
                    r#"{"choices":[{"delta":null,"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#,
                ],
                Expected::Incomplete {
                    metadata: Some(
                        json!({"protocol":"chat_completions","provider":"openai","stop_reason":"stop","usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}),
                    ),
                },
            ),
            (
                TestProvider::Anthropic,
                &[
                    r#"{"type":"message_start","message":{"usage":{"input_tokens":2,"output_tokens":0}}}"#,
                    r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}"#,
                    r#"{"type":"content_block_stop","index":0}"#,
                    r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}"#,
                ],
                Expected::Incomplete {
                    metadata: Some(
                        json!({"protocol":"anthropic","provider":"anthropic","stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}),
                    ),
                },
            ),
            (
                TestProvider::Responses,
                &[
                    r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"partial"}"#,
                ],
                // 只有已解码正文、没有response外层原件；D2保正文但不能补造观察。
                Expected::Incomplete { metadata: None },
            ),
        ];
        for (provider, events, expected) in cases {
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
            assert_eq!(items.iter().filter(|item| item.is_err()).count(), 1);
            let error = items
                .iter()
                .find_map(|item| item.as_ref().err())
                .ok_or_else(|| {
                    LlmError::InvalidResponse(format!(
                        "{} omitted incomplete failure",
                        provider.name()
                    ))
                })?;
            let Expected::Incomplete { metadata } = expected;
            let echo_core::error::ReactError::Llm(error) = error else {
                return Err(LlmError::InvalidResponse(
                    "expected LlmError::IncompleteResponse".into(),
                )
                .into());
            };
            let LlmError::IncompleteResponse { partial } = error.as_ref() else {
                return Err(LlmError::InvalidResponse(format!(
                    "{} returned a non-D2 error",
                    provider.name()
                ))
                .into());
            };
            assert_eq!(partial.content.as_text().as_deref(), Some("partial"));
            assert!(
                partial.approved_assistant_blocks.is_none() && partial.execution_status.is_none()
            );
            let observed = partial
                .provider_response
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|error| LlmError::SerializationError(error.to_string()))?;
            assert_eq!(
                observed,
                metadata.clone(),
                "{} changed native partial observation",
                provider.name()
            );
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
                    r#"{"id":"chat-success","choices":[{"delta":{"content":"done"},"index":0}]}"#,
                    r#"{"id":"chat-success","choices":[{"delta":null,"index":0,"finish_reason":"stop"}]}"#,
                    r#"{"id":"chat-success","choices":[],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#,
                    "[DONE]",
                ],
            ),
            (
                TestProvider::Anthropic,
                &[
                    r#"{"type":"message_start","message":{"id":"anthropic-native-complete","usage":{"input_tokens":2,"output_tokens":0}}}"#,
                    r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"done"}}"#,
                    r#"{"type":"content_block_stop","index":0}"#,
                    r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}"#,
                    r#"{"type":"message_stop"}"#,
                ],
            ),
            (
                TestProvider::Responses,
                &[
                    r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"done"}"#,
                    r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"done"}]}],"usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}}"#,
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
        use serde_json::{Value, json};
        enum Expected {
            Incomplete {
                content: &'static str,
                metadata: Value,
            },
            Empty,
            Invalid,
        }
        let cases: &[(TestProvider, &[&str], Expected)] = &[
            (
                TestProvider::OpenAi,
                &[
                    r#"{"choices":[{"delta":{"content":"partial"},"index":0}]}"#,
                    "[DONE]",
                ],
                Expected::Incomplete {
                    content: "partial",
                    metadata: json!({"protocol":"chat_completions","provider":"openai"}),
                },
            ),
            (
                TestProvider::OpenAi,
                &[
                    r#"{"choices":[{"delta":{"content":"partial"},"index":0}]}"#,
                    r#"{"choices":[{"delta":null,"index":0,"finish_reason":"length"}]}"#,
                    "[DONE]",
                ],
                Expected::Incomplete {
                    content: "partial",
                    metadata: json!({"protocol":"chat_completions","provider":"openai","stop_reason":"length"}),
                },
            ),
            (
                TestProvider::OpenAi,
                &[
                    r#"{"choices":[{"delta":null,"index":0,"finish_reason":"stop"}]}"#,
                    r#"{"choices":[{"delta":{"content":"late"},"index":0}]}"#,
                    "[DONE]",
                ],
                Expected::Invalid,
            ),
            (
                TestProvider::Anthropic,
                &[
                    r#"{"type":"message_start","message":{"usage":{"input_tokens":2,"output_tokens":0}}}"#,
                    r#"{"type":"message_stop"}"#,
                ],
                Expected::Empty,
            ),
            (
                TestProvider::Anthropic,
                &[
                    r#"{"type":"message_start","message":{"usage":{"input_tokens":2,"output_tokens":0}}}"#,
                    r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":1}}"#,
                    r#"{"type":"message_stop"}"#,
                ],
                Expected::Incomplete {
                    content: "",
                    metadata: json!({"protocol":"anthropic","provider":"anthropic","stop_reason":"max_tokens","usage":{"input_tokens":2,"output_tokens":1}}),
                },
            ),
            (TestProvider::Responses, &["[DONE]"], Expected::Invalid),
        ];
        for (provider, events, expected) in cases {
            let items = collect_provider_sse(*provider, events).await?;
            assert_eq!(items.iter().filter(|item| item.is_err()).count(), 1);
            let error = items
                .iter()
                .find_map(|item| item.as_ref().err())
                .ok_or_else(|| {
                    LlmError::InvalidResponse(format!(
                        "{} omitted invalid-terminal failure",
                        provider.name()
                    ))
                })?;
            match expected {
                Expected::Incomplete { content, metadata } => {
                    let echo_core::error::ReactError::Llm(error) = error else {
                        return Err(LlmError::InvalidResponse(
                            "expected LlmError::IncompleteResponse".into(),
                        )
                        .into());
                    };
                    let LlmError::IncompleteResponse { partial } = error.as_ref() else {
                        return Err(LlmError::InvalidResponse(format!(
                            "{} returned a non-D2 error",
                            provider.name()
                        ))
                        .into());
                    };
                    if content.is_empty() {
                        assert!(
                            partial
                                .content
                                .as_text()
                                .as_deref()
                                .is_none_or(str::is_empty)
                        );
                    } else {
                        assert_eq!(partial.content.as_text().as_deref(), Some(*content));
                    }
                    assert!(
                        partial.approved_assistant_blocks.is_none()
                            && partial.execution_status.is_none()
                    );
                    let observed = partial.provider_response.as_ref().ok_or_else(|| {
                        LlmError::InvalidResponse("D2 omitted decoded native observation".into())
                    })?;
                    assert_eq!(
                        serde_json::to_value(observed)
                            .map_err(|error| LlmError::SerializationError(error.to_string()))?,
                        metadata.clone()
                    );
                }
                Expected::Empty => {
                    assert!(
                        matches!(error, echo_core::error::ReactError::Llm(error) if matches!(error.as_ref(), LlmError::EmptyResponse)),
                        "{} returned a non-empty error",
                        provider.name()
                    );
                }
                Expected::Invalid => {
                    assert!(
                        matches!(error, echo_core::error::ReactError::Llm(error) if matches!(error.as_ref(), LlmError::InvalidResponse(_))),
                        "{} returned a non-protocol error",
                        provider.name()
                    );
                }
            }
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
    #[tokio::test]
    async fn chat_provider_error_before_or_after_finish_never_publishes_success() -> Result<()> {
        let error = r#"{"error":{"type":"server_error","code":"upstream_failed","message":"原服务错误🙂"}}"#;
        let progress = r#"{"choices":[{"delta":{"content":"partial"},"index":0}]}"#;
        let finish = r#"{"choices":[{"delta":null,"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#;
        for events in [
            [progress, error, finish, "[DONE]"],
            [progress, finish, error, "[DONE]"],
        ] {
            let items = collect_provider_sse(TestProvider::OpenAi, &events).await?;
            let error = items
                .iter()
                .find_map(|item| item.as_ref().err())
                .ok_or_else(|| LlmError::InvalidResponse("provider error was swallowed".into()))?;
            let failure = echo_core::error::AgentFailure::try_from(error)?;
            assert_eq!(failure.code, "llm_provider");
            assert!(!failure.retryable);
            assert_eq!(failure.http_status, None);
            assert!(
                failure.message.contains("upstream_failed")
                    && failure.message.contains("原服务错误🙂")
            );
            assert!(
                items
                    .iter()
                    .filter_map(|item| item.as_ref().ok())
                    .all(|chunk| chunk.finish_reason.is_none() && chunk.usage.is_none()),
                "provider error不能发布成功finish/usage"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn anthropic_signed_stream_error_preserves_payload_and_does_not_finish() -> Result<()> {
        let events = [
            r#"{"type":"message_start","message":{"id":"original-signed-request","usage":{"input_tokens":2,"output_tokens":0}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"原生推理"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signature-A"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"-B"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"partial"}}"#,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"provider busy 原始说明"}}"#,
        ];
        let items = collect_provider_sse(TestProvider::Anthropic, &events).await?;
        let signed = items
            .iter()
            .filter_map(|item| item.as_ref().ok())
            .flat_map(|chunk| chunk.delta.assistant_blocks.iter().flatten())
            .filter_map(|block| block.reasoning.as_ref());
        assert!(signed.into_iter().any(|block| matches!(block,
            echo_core::llm::types::ReasoningBlock::Signed {thinking,signature}
                if thinking == "原生推理" && signature == "signature-A-B")));
        let error = items
            .iter()
            .find_map(|item| item.as_ref().err())
            .ok_or_else(|| LlmError::InvalidResponse("provider error omitted".into()))?;
        let failure = echo_core::error::AgentFailure::try_from(error)?;
        assert_eq!(failure.code, "llm_provider");
        assert!(!failure.retryable);
        assert_eq!(failure.http_status, None);
        assert!(
            failure.message.contains("overloaded_error")
                && failure.message.contains("provider busy 原始说明")
        );
        assert!(
            items
                .iter()
                .filter_map(|item| item.as_ref().ok())
                .all(|chunk| chunk.finish_reason.is_none() && chunk.usage.is_none())
        );
        Ok(())
    }

    #[tokio::test]
    async fn chat_reasoning_usage_survives_actual_json_and_final_choice_sse() -> Result<()> {
        let usage = serde_json::json!({"prompt_tokens":11,"completion_tokens":13,"total_tokens":24,
            "prompt_cache_hit_tokens":7,"prompt_cache_miss_tokens":4,
            "completion_tokens_details":{"reasoning_tokens":9}});
        let response = collect_provider_json(TestProvider::OpenAi, serde_json::json!({
            "id":"request-usage","choices":[{"index":0,"message":{"role":"assistant","content":"answer"},"finish_reason":"stop"}],
            "usage":usage.clone()
        })).await?;
        let actual = response
            .usage
            .as_ref()
            .ok_or_else(|| LlmError::InvalidResponse("missing JSON usage".into()))?;
        assert_eq!(
            actual
                .output_tokens_details
                .as_ref()
                .and_then(|details| details.reasoning_tokens),
            Some(9)
        );
        assert_eq!(actual.prompt_cache_hit_tokens, Some(7));
        assert_eq!(actual.prompt_cache_miss_tokens, Some(4));
        assert_eq!(actual.total_tokens, Some(24));
        let progress = serde_json::json!({"id":"request-usage","choices":[{"index":0,"delta":{"content":"answer"}}]}).to_string();
        let terminal = serde_json::json!({"id":"request-usage","choices":[{"index":0,"delta":null,"finish_reason":"stop"}],"usage":usage}).to_string();
        let items = collect_provider_sse(
            TestProvider::OpenAi,
            &[progress.as_str(), terminal.as_str(), "[DONE]"],
        )
        .await?;
        assert!(items.iter().all(Result::is_ok));
        let finals = items
            .iter()
            .filter_map(|item| item.as_ref().ok())
            .filter(|chunk| chunk.finish_reason.is_some())
            .collect::<Vec<_>>();
        assert_eq!(finals.len(), 1);
        let actual = finals
            .first()
            .and_then(|chunk| chunk.usage.as_ref())
            .ok_or_else(|| LlmError::InvalidResponse("missing final SSE usage".into()))?;
        assert_eq!(
            actual
                .output_tokens_details
                .as_ref()
                .and_then(|details| details.reasoning_tokens),
            Some(9)
        );
        assert_eq!(actual.prompt_cache_hit_tokens, Some(7));
        assert_eq!(actual.prompt_cache_miss_tokens, Some(4));
        assert_eq!(actual.total_tokens, Some(24));
        Ok(())
    }

    #[tokio::test]
    async fn responses_plaintext_reasoning_survives_actual_json_and_sse() -> Result<()> {
        let response = serde_json::json!({"id":"plain-http-response","status":"completed","output":[
            {"id":"plain-http-reason","type":"reasoning","content":[{"type":"reasoning_text","text":"明文推理🙂"}]},
            {"id":"plain-http-answer","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"answer"}]}]});
        let converted = collect_provider_json(TestProvider::Responses, response.clone()).await?;
        assert_eq!(
            converted.message.reasoning_content.as_deref(),
            Some("明文推理🙂")
        );
        assert!(converted.message.reasoning_blocks.is_none());
        let created =
            serde_json::json!({"type":"response.created","response":{"id":"plain-http-response"}})
                .to_string();
        let delta=serde_json::json!({"type":"response.reasoning_text.delta","item_id":"plain-http-reason","output_index":0,"content_index":0,"delta":"明文"}).to_string();
        let done=serde_json::json!({"type":"response.output_item.done","output_index":0,"item":response.pointer("/output/0").cloned().ok_or_else(||LlmError::InvalidResponse("missing fixture reasoning".into()))?}).to_string();
        let complete =
            serde_json::json!({"type":"response.completed","response":response}).to_string();
        let items = collect_provider_sse(
            TestProvider::Responses,
            &[
                created.as_str(),
                delta.as_str(),
                done.as_str(),
                complete.as_str(),
            ],
        )
        .await?;
        assert!(items.iter().all(Result::is_ok));
        let chunks = items
            .iter()
            .filter_map(|item| item.as_ref().ok())
            .collect::<Vec<_>>();
        let reasoning: String = chunks
            .iter()
            .flat_map(|chunk| chunk.delta.assistant_blocks.iter().flatten())
            .filter(|block| block.block_id == "plain-http-reason:content:0")
            .map(|block| block.delta.as_str())
            .collect();
        assert_eq!(reasoning, "明文推理🙂");
        assert_eq!(
            chunks
                .iter()
                .filter(|chunk| chunk.finish_reason.is_some())
                .count(),
            1
        );
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.delta.reasoning_blocks.is_none())
        );
        Ok(())
    }
    #[tokio::test]
    async fn all_provider_sse_errors_are_typed_nonretryable_without_synthetic_http() -> Result<()> {
        for (provider, events) in [
            (
                TestProvider::OpenAi,
                vec![
                    r#"{"error":{"type":"rate_limit_error","code":null,"message":"429 overloaded words are not retry permission"}}"#,
                    "[DONE]",
                ],
            ),
            (
                TestProvider::Anthropic,
                vec![
                    r#"{"type":"error","error":{"type":"overloaded_error","code":37,"message":"original"}}"#,
                ],
            ),
            (
                TestProvider::Responses,
                vec![r#"{"type":"error","code":"server_error","message":"original"}"#],
            ),
            (
                TestProvider::Responses,
                vec![
                    r#"{"type":"response.failed","response":{"id":"resp_failed","status":"failed","error":{"code":"server_error","message":"original"}}}"#,
                ],
            ),
        ] {
            let items = collect_provider_sse(provider, &events).await?;
            let error = items
                .iter()
                .find_map(|item| item.as_ref().err())
                .ok_or_else(|| LlmError::InvalidResponse("SSE error was swallowed".into()))?;
            let failure = echo_core::error::AgentFailure::try_from(error)?;
            assert_eq!(failure.code, "llm_provider");
            assert!(!failure.retryable);
            assert_eq!(failure.http_status, None);
            assert!(
                items
                    .iter()
                    .filter_map(|item| item.as_ref().ok())
                    .all(|chunk| chunk.finish_reason.is_none() && chunk.usage.is_none())
            );
        }
        Ok(())
    }

    #[test]
    fn provider_error_code_missing_null_and_numeric_are_not_coerced() {
        for code in [
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::json!(37)),
        ] {
            let mut payload =
                serde_json::json!({"type":"unknown_error","message":"overloaded rate limit 429"});
            if let Some(value) = &code
                && let Some(object) = payload.as_object_mut()
            {
                object.insert("code".into(), value.clone());
            }
            let error = reported_provider_error("custom-provider", &payload);
            assert!(
                matches!(error,LlmError::ProviderError {provider,error_type,code:actual,message,retryable:false}
                if provider=="custom-provider" && error_type.as_deref()==Some("unknown_error") && actual==code && message=="overloaded rate limit 429")
            );
        }
    }
}

// 同一片段追加到before/after的原client.rs；没有引用候选helper，不改当前源码。
#[cfg(test)]
mod stream_deadline_input_tests {
    use super::*;
    use echo_core::error::{ConfigError, ReactError};
    use echo_core::llm::{ChatRequest, LlmClient, Message};
    use futures::FutureExt;
    use std::panic::AssertUnwindSafe;

    fn cannot_send_request() -> Result<RequestBuilder> {
        let client = Client::new();
        if client.post("http://[").build().is_ok() {
            return Err(ReactError::Other(
                "invalid URL fixture unexpectedly built".into(),
            ));
        }
        // URL解析错误保存在原RequestBuilder，send只能返回builder错误，没有TCP/DNS。
        Ok(client.post("http://["))
    }

    async fn boundary_error(
        timeouts: LlmTimeouts,
        token: Option<CancellationToken>,
    ) -> Result<ReactError> {
        let future = stream_json_sse(cannot_send_request()?, "test-model".into(), timeouts, token);
        match AssertUnwindSafe(future).catch_unwind().await {
            Ok(Err(error)) => Ok(error),
            Ok(Ok(_)) => Err(ReactError::Other(
                "invalid request unexpectedly returned stream".into(),
            )),
            Err(_) => Err(ReactError::Other(
                "existing stream deadline panicked before returning typed error".into(),
            )),
        }
    }

    fn require_config(error: ReactError, phase: &str) -> Result<()> {
        match error {
            ReactError::Config(inner) => match inner.as_ref() {
                ConfigError::ConfigFileError(message)
                    if message.contains(phase) && message.contains("cannot be represented") =>
                {
                    Ok(())
                }
                _ => Err(ReactError::Other(format!(
                    "wrong configuration error: {inner}"
                ))),
            },
            other => Err(ReactError::Other(format!(
                "expected {phase} configuration error, got {other}"
            ))),
        }
    }

    fn require_original_builder_error(error: ReactError) -> Result<()> {
        match error {
            ReactError::Llm(inner) if matches!(inner.as_ref(), LlmError::NetworkError(message) if message.contains("builder")) => {
                Ok(())
            }
            other => Err(ReactError::Other(format!(
                "original builder failure changed: {other}"
            ))),
        }
    }

    #[tokio::test]
    async fn overall_duration_max_is_typed_config_before_transport() -> Result<()> {
        require_config(
            boundary_error(
                LlmTimeouts::default().with_overall_timeout(Duration::MAX),
                None,
            )
            .await?,
            "overall",
        )
    }

    #[tokio::test]
    async fn first_chunk_duration_max_is_typed_config_before_transport() -> Result<()> {
        require_config(
            boundary_error(
                LlmTimeouts::default().with_first_chunk_timeout(Duration::MAX),
                None,
            )
            .await?,
            "first chunk",
        )
    }

    #[tokio::test]
    async fn idle_duration_max_is_typed_config_before_transport() -> Result<()> {
        require_config(
            boundary_error(
                LlmTimeouts::default().with_idle_timeout(Duration::MAX),
                None,
            )
            .await?,
            "idle",
        )
    }

    #[tokio::test]
    async fn disabled_zero_and_valid_timeouts_keep_original_builder_failure() -> Result<()> {
        for timeouts in [
            LlmTimeouts::default(),
            LlmTimeouts::default()
                .without_first_chunk_timeout()
                .without_idle_timeout()
                .without_overall_timeout(),
            LlmTimeouts::default()
                .with_first_chunk_timeout(Duration::ZERO)
                .with_idle_timeout(Duration::ZERO)
                .with_overall_timeout(Duration::ZERO),
        ] {
            require_original_builder_error(boundary_error(timeouts, None).await?)?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_request_precedes_extreme_timeout_and_builder_error() -> Result<()> {
        let token = CancellationToken::new();
        token.cancel();
        let error = boundary_error(
            LlmTimeouts::default()
                .with_overall_timeout(Duration::MAX)
                .with_first_chunk_timeout(Duration::MAX)
                .with_idle_timeout(Duration::MAX),
            Some(token),
        )
        .await?;
        match error {
            ReactError::Llm(inner) if matches!(inner.as_ref(), LlmError::NetworkError(message) if message == "LLM stream cancelled") => {
                Ok(())
            }
            other => Err(ReactError::Other(format!(
                "cancellation first cause changed: {other}"
            ))),
        }
    }

    #[tokio::test]
    async fn decoded_u64_max_millis_are_retained_and_checked_without_arbitrary_cap() -> Result<()> {
        for field in ["overall", "first_chunk", "idle"] {
            let mut map = serde_json::Map::new();
            for name in ["request", "first_chunk", "idle", "overall"] {
                map.insert(name.into(), serde_json::Value::Null);
            }
            map.insert(field.into(), serde_json::Value::from(u64::MAX));
            let timeouts: LlmTimeouts = serde_json::from_value(serde_json::Value::Object(map))
                .map_err(|error| LlmError::SerializationError(error.to_string()))?;
            let duration = match field {
                "overall" => timeouts.overall_timeout(),
                "first_chunk" => timeouts.first_chunk_timeout(),
                _ => timeouts.idle_timeout(),
            }
            .ok_or_else(|| {
                ReactError::Other("large decoded duration was silently disabled".into())
            })?;
            assert_eq!(duration, Duration::from_millis(u64::MAX));
            let representable = tokio::time::Instant::now().checked_add(duration).is_some();
            let error = boundary_error(timeouts, None).await?;
            if representable {
                require_original_builder_error(error)?;
            } else {
                require_config(
                    error,
                    if field == "first_chunk" {
                        "first chunk"
                    } else {
                        field
                    },
                )?;
            }
        }
        Ok(())
    }

    async fn anthropic_error(
        client_timeouts: LlmTimeouts,
        override_timeouts: Option<LlmTimeouts>,
    ) -> Result<ReactError> {
        let client = super::super::anthropic::AnthropicClient::with_base_url(
            "http://[",
            "test-key",
            "claude-sonnet-4-6",
        )
        .with_timeouts(client_timeouts);
        let mut request = ChatRequest::new(vec![Message::user("hello".into())]);
        request.timeouts = override_timeouts;
        match AssertUnwindSafe(client.chat_stream(request))
            .catch_unwind()
            .await
        {
            Ok(Err(error)) => Ok(error),
            Ok(Ok(_)) => Err(ReactError::Other(
                "invalid public client URL unexpectedly returned stream".into(),
            )),
            Err(_) => Err(ReactError::Other(
                "existing public client timeout panicked".into(),
            )),
        }
    }

    #[tokio::test]
    async fn public_request_override_replaces_invalid_client_timeout() -> Result<()> {
        require_original_builder_error(
            anthropic_error(
                LlmTimeouts::default().with_overall_timeout(Duration::MAX),
                Some(LlmTimeouts::default()),
            )
            .await?,
        )
    }

    #[tokio::test]
    async fn public_request_extreme_override_returns_typed_config() -> Result<()> {
        require_config(
            anthropic_error(
                LlmTimeouts::default(),
                Some(LlmTimeouts::default().with_overall_timeout(Duration::MAX)),
            )
            .await?,
            "overall",
        )
    }

    #[tokio::test]
    async fn original_decoder_rejects_unrepresentable_idle_wait() -> Result<()> {
        let source = futures::stream::pending::<std::result::Result<Vec<u8>, std::io::Error>>();
        let raw = decode_json_sse_bytes(source, Vec::new(), Some(Duration::MAX), None, None, None);
        futures::pin_mut!(raw);
        let outcome = tokio::time::timeout(
            Duration::from_millis(100),
            AssertUnwindSafe(raw.next()).catch_unwind(),
        )
        .await;
        match outcome {
            Ok(Ok(Some(Err(error)))) => require_config(error, "idle"),
            Ok(Err(_)) => Err(ReactError::Other("decoder idle deadline panicked".into())),
            _ => Err(ReactError::Other(
                "unrepresentable idle duration silently became a far-future wait".into(),
            )),
        }
    }

    #[tokio::test]
    async fn original_decoder_preserves_valid_idle_and_overall_timeout_errors() -> Result<()> {
        let duration = Duration::from_millis(20);
        for overall in [false, true] {
            let source = futures::stream::pending::<std::result::Result<Vec<u8>, std::io::Error>>();
            let deadline = tokio::time::Instant::now()
                .checked_add(duration)
                .ok_or_else(|| ReactError::Other("small valid deadline unavailable".into()))?;
            let raw = decode_json_sse_bytes(
                source,
                Vec::new(),
                (!overall).then_some(duration),
                overall.then_some(duration),
                overall.then_some(deadline),
                None,
            );
            futures::pin_mut!(raw);
            let next = raw
                .next()
                .await
                .ok_or_else(|| ReactError::Other("parked source ended".into()))?;
            match next {
                Err(ReactError::Llm(inner))
                    if matches!(inner.as_ref(), LlmError::NetworkError(message)
                    if message.contains(if overall { "overall timeout after 20ms" } else { "idle timeout after 20ms" })) =>
                    {}
                Err(other) => {
                    return Err(ReactError::Other(format!(
                        "valid timeout error changed: {other}"
                    )));
                }
                Ok(_) => return Err(ReactError::Other("parked source emitted an event".into())),
            }
        }
        Ok(())
    }
}
