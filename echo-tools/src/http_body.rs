use echo_core::error::{ReactError, ToolError};
use echo_core::tools::ToolContext;
use futures::StreamExt;
#[cfg(any(feature = "web", feature = "research"))]
use serde::de::DeserializeOwned;

#[cfg(any(feature = "web", feature = "research"))]
pub(crate) const MAX_API_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) async fn read_bounded_body(
    response: reqwest::Response,
    max_bytes: usize,
    tool_name: &str,
    context: Option<&ToolContext>,
) -> Result<Vec<u8>, ReactError> {
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
        let chunk = chunk.map_err(|error| ToolError::ExecutionFailed {
            tool: tool_name.to_string(),
            message: format!("Failed to read response body: {error}"),
        })?;
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

#[cfg(any(feature = "web", feature = "research"))]
pub(crate) async fn read_bounded_text(
    response: reqwest::Response,
    max_bytes: usize,
    tool_name: &str,
    context: Option<&ToolContext>,
) -> Result<String, ReactError> {
    let bytes = read_bounded_body(response, max_bytes, tool_name, context).await?;
    String::from_utf8(bytes).map_err(|error| {
        ToolError::ExecutionFailed {
            tool: tool_name.to_string(),
            message: format!("Response body is not valid UTF-8: {error}"),
        }
        .into()
    })
}

#[cfg(any(feature = "web", feature = "research"))]
pub(crate) async fn read_bounded_json<T: DeserializeOwned>(
    response: reqwest::Response,
    max_bytes: usize,
    tool_name: &str,
    context: Option<&ToolContext>,
) -> Result<T, ReactError> {
    let bytes = read_bounded_body(response, max_bytes, tool_name, context).await?;
    serde_json::from_slice(&bytes).map_err(|error| {
        ToolError::ExecutionFailed {
            tool: tool_name.to_string(),
            message: format!("Failed to parse response JSON: {error}"),
        }
        .into()
    })
}
