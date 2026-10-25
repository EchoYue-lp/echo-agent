use echo_core::error::{ReactError, ToolError};
use echo_core::tools::{ToolContext, ToolFailure, ToolFailureCategory};
use futures::StreamExt;
#[cfg(any(feature = "web", feature = "research"))]
use serde::de::DeserializeOwned;

#[cfg(any(feature = "web", feature = "research"))]
pub(crate) const MAX_API_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn remote_failure(
    tool_name: &str,
    message: impl Into<String>,
    http_status: Option<u16>,
    failure: ToolFailure,
) -> ReactError {
    ToolError::RemoteFailure {
        tool: tool_name.to_string(),
        message: message.into(),
        http_status,
        failure: Box::new(failure),
    }
    .into()
}

/// Normalize transport facts before a reqwest error is reduced to text.
/// Strip URLs because even a local application's model/logs must not receive
/// credentials or sensitive query parameters from a diagnostic.
pub(crate) fn request_error(
    tool_name: &str,
    error: reqwest::Error,
    http_status: Option<u16>,
) -> ReactError {
    if error.is_timeout() {
        return ToolError::Timeout(tool_name.to_string()).into();
    }
    let category = if error.is_connect() || error.is_body() {
        ToolFailureCategory::Transient
    } else {
        ToolFailureCategory::Permanent
    };
    remote_failure(
        tool_name,
        format!("Remote request failed: {}", error.without_url()),
        http_status,
        ToolFailure::new(category),
    )
}

#[cfg(feature = "web")]
pub(crate) fn response_error(tool_name: &str, response: &reqwest::Response) -> ReactError {
    let status = response.status().as_u16();
    let category = match status {
        408 => ToolFailureCategory::Timeout,
        429 | 500..=599 => ToolFailureCategory::Transient,
        _ => ToolFailureCategory::Permanent,
    };
    let mut failure = ToolFailure::new(category);
    if matches!(
        category,
        ToolFailureCategory::Timeout | ToolFailureCategory::Transient
    ) {
        failure = failure.retryable();
    }
    let values = response.headers().get_all(reqwest::header::RETRY_AFTER);
    let mut values = values.iter();
    if let Some(value) = values.next()
        && values.all(|other| other == value)
    {
        failure.retry_after_ms = value
            .to_str()
            .ok()
            .and_then(|value| retry_after_ms(value, std::time::SystemTime::now()));
    }
    remote_failure(
        tool_name,
        format!("Remote HTTP status {status}"),
        Some(status),
        failure,
    )
}

#[cfg(feature = "web")]
fn retry_after_ms(value: &str, observed: std::time::SystemTime) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.chars().all(|character| character.is_ascii_digit()) {
        return value.parse::<u64>().ok()?.checked_mul(1_000);
    }
    let date = httpdate::parse_http_date(value).ok()?;
    let duration = date.duration_since(observed).unwrap_or_default();
    // Milliseconds are a lower-bound wait; flooring a fractional millisecond
    // could permit a request before the server's absolute HTTP-date.
    let rounded =
        duration
            .as_millis()
            .checked_add(if duration.subsec_nanos().is_multiple_of(1_000_000) {
                0
            } else {
                1
            })?;
    u64::try_from(rounded).ok()
}

pub(crate) async fn read_bounded_body(
    response: reqwest::Response,
    max_bytes: usize,
    tool_name: &str,
    context: Option<&ToolContext>,
) -> Result<Vec<u8>, ReactError> {
    let http_status = response.status().as_u16();
    if let Some(length) = response.content_length()
        && length > u64::try_from(max_bytes).unwrap_or(u64::MAX)
    {
        return Err(ToolError::FileTooLarge {
            size: length,
            max: u64::try_from(max_bytes).unwrap_or(u64::MAX),
        }
        .into());
    }

    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0)
            .min(max_bytes),
    );
    let mut stream = response.bytes_stream();
    loop {
        let next = if let Some(cancel) = context.and_then(|value| value.cancel.as_ref()) {
            tokio::select! {
                _ = cancel.cancelled() => {
                    return Err(echo_core::error::AgentError::Cancelled(
                        format!("tool '{tool_name}' response body"),
                    ).into());
                }
                chunk = stream.next() => chunk,
            }
        } else {
            stream.next().await
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(|error| request_error(tool_name, error, Some(http_status)))?;
        let next_len =
            bytes
                .len()
                .checked_add(chunk.len())
                .ok_or_else(|| ToolError::FileTooLarge {
                    size: u64::MAX,
                    max: u64::try_from(max_bytes).unwrap_or(u64::MAX),
                })?;
        if next_len > max_bytes {
            return Err(ToolError::FileTooLarge {
                size: u64::try_from(next_len).unwrap_or(u64::MAX),
                max: u64::try_from(max_bytes).unwrap_or(u64::MAX),
            }
            .into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(any(feature = "research", all(test, feature = "web")))]
pub(crate) async fn read_bounded_text(
    response: reqwest::Response,
    max_bytes: usize,
    tool_name: &str,
    context: Option<&ToolContext>,
) -> Result<String, ReactError> {
    let status = response.status().as_u16();
    let bytes = read_bounded_body(response, max_bytes, tool_name, context).await?;
    String::from_utf8(bytes).map_err(|_| {
        remote_failure(
            tool_name,
            "Response body is not valid UTF-8",
            Some(status),
            ToolFailure::new(ToolFailureCategory::Permanent),
        )
    })
}

#[cfg(any(feature = "web", feature = "research"))]
pub(crate) async fn read_bounded_json<T: DeserializeOwned>(
    response: reqwest::Response,
    max_bytes: usize,
    tool_name: &str,
    context: Option<&ToolContext>,
) -> Result<T, ReactError> {
    let status = response.status().as_u16();
    let bytes = read_bounded_body(response, max_bytes, tool_name, context).await?;
    serde_json::from_slice(&bytes).map_err(|_| {
        remote_failure(
            tool_name,
            "Response body is not valid JSON for this operation",
            Some(status),
            ToolFailure::new(ToolFailureCategory::Permanent),
        )
    })
}

#[cfg(all(test, feature = "web"))]
pub(crate) mod test_http {
    use std::io::{self, Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    pub(crate) struct CapturedRequest {
        pub path: String,
        pub at: Instant,
    }

    pub(crate) fn client() -> Result<reqwest::Client, echo_core::error::ReactError> {
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|error| super::request_error("web_search", error, None))
    }

    pub(crate) fn response(status: u16, headers: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        ).into_bytes()
    }

    pub(crate) fn server(
        responses: Vec<Vec<u8>>,
    ) -> io::Result<(String, JoinHandle<io::Result<Vec<CapturedRequest>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        listener.set_nonblocking(true)?;
        let server = std::thread::Builder::new()
            .name("web-failure-fixture".to_string())
            .spawn(move || {
                let deadline = Instant::now()
                    .checked_add(Duration::from_secs(3))
                    .ok_or_else(|| io::Error::other("fixture deadline overflow"))?;
                let mut captured = Vec::new();
                for response in responses {
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                                if Instant::now() >= deadline {
                                    return Err(io::Error::new(
                                        io::ErrorKind::TimedOut,
                                        "expected HTTP request was not received",
                                    ));
                                }
                                std::thread::sleep(Duration::from_millis(1));
                            }
                            Err(error) => return Err(error),
                        }
                    };
                    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                    let mut request = Vec::new();
                    loop {
                        let mut chunk = [0_u8; 2_048];
                        let count = stream.read(&mut chunk)?;
                        if count == 0 {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "request headers were incomplete",
                            ));
                        }
                        request.extend_from_slice(chunk.get(..count).unwrap_or_default());
                        if request.len() > 64 * 1024 {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "fixture request too large",
                            ));
                        }
                        if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                            let body_start = end.saturating_add(4);
                            let headers = String::from_utf8_lossy(
                                request.get(..body_start).unwrap_or_default(),
                            );
                            let length = headers
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                                .unwrap_or(0);
                            if request.len().saturating_sub(body_start) >= length {
                                break;
                            }
                        }
                    }
                    let text = String::from_utf8_lossy(&request);
                    captured.push(CapturedRequest {
                        path: text
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or_default()
                            .to_string(),
                        at: Instant::now(),
                    });
                    stream.write_all(&response)?;
                }
                Ok(captured)
            })?;
        Ok((endpoint, server))
    }
}

#[cfg(all(test, feature = "web"))]
mod retry_after_tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[tokio::test]
    async fn original_bounded_reader_keeps_size_and_utf8_errors() -> Result<(), ReactError> {
        use super::test_http::{response, server};
        for (raw, limit, oversized) in [
            (response(200, "", "four"), 3, true),
            (
                b"HTTP/1.1 200 Fixture\r\nContent-Length: 1\r\nConnection: close\r\n\r\n\xff"
                    .to_vec(),
                16,
                false,
            ),
        ] {
            let (endpoint, serving) = server(vec![raw])?;
            let response = super::test_http::client()?
                .get(&endpoint)
                .send()
                .await
                .map_err(|error| request_error("web_search", error, None))?;
            let error = read_bounded_text(response, limit, "web_search", None)
                .await
                .err()
                .ok_or_else(|| ReactError::Other("invalid body returned success".to_string()))?;
            if oversized {
                assert!(
                    matches!(error, ReactError::Tool(ref error) if matches!(error.as_ref(), ToolError::FileTooLarge { .. }))
                );
            } else {
                assert_eq!(
                    ToolFailure::from_error(&error, false).category,
                    ToolFailureCategory::Permanent
                );
            }
            assert_eq!(
                serving
                    .join()
                    .map_err(|_| ReactError::Other("HTTP fixture panicked".to_string()))??
                    .len(),
                1
            );
        }
        Ok(())
    }

    #[test]
    fn retry_after_preserves_zero_dates_and_rejects_invalid_values() {
        let observed = UNIX_EPOCH + Duration::from_secs(1_445_412_480);
        assert_eq!(retry_after_ms("0", observed), Some(0));
        assert_eq!(retry_after_ms("2", observed), Some(2_000));
        assert_eq!(
            retry_after_ms("Wed, 21 Oct 2015 07:28:02 GMT", observed),
            Some(2_000)
        );
        assert_eq!(
            retry_after_ms(
                "Wed, 21 Oct 2015 07:28:02 GMT",
                observed + Duration::from_nanos(1)
            ),
            Some(2_000)
        );
        assert_eq!(
            retry_after_ms("Wed, 21 Oct 2015 07:27:00 GMT", observed),
            Some(0)
        );
        for invalid in ["", "-1", "+2", "invalid", "18446744073709551615"] {
            assert_eq!(retry_after_ms(invalid, observed), None);
        }
    }
}
