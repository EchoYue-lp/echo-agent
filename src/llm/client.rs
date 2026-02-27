use crate::error::{LlmError, Result};
use crate::llm::ChatCompletionRequest;
use crate::llm::types::{ChatCompletionChunk, ChatCompletionResponse};
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use tracing::debug;

pub async fn post(
    client: Arc<Client>,
    request_body: &ChatCompletionRequest,
    header_map: HeaderMap,
    url: &str,
) -> Result<ChatCompletionResponse> {
    debug!(
        "Post completion request_body: {}",
        serde_json::to_string(request_body).unwrap_or_else(|e| format!("<serialize error: {}>", e))
    );
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

    debug!(
        "Post completion response: {}",
        serde_json::to_string(&completion_response)
            .unwrap_or_else(|e| format!("<serialize error: {}>", e))
    );

    Ok(completion_response)
}

/// 发送带 `stream: true` 的请求，返回解析好的 SSE chunk 流
///
/// 注意：接收 `request_body` 的所有权，避免引用与 async stream 的生命周期冲突。
pub async fn stream_post(
    client: Arc<Client>,
    request_body: ChatCompletionRequest,
    header_map: HeaderMap,
    url: String,
) -> Result<impl Stream<Item = Result<ChatCompletionChunk>>> {
    debug!(
        "Stream completion request_body: {}",
        serde_json::to_string(&request_body)
            .unwrap_or_else(|e| format!("<serialize error: {}>", e))
    );

    let response = client
        .post(&url)
        .headers(header_map)
        .json(&request_body)
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

    let byte_stream = response.bytes_stream();

    let stream = async_stream::try_stream! {
        let mut buffer = String::new();
        tokio::pin!(byte_stream);

        while let Some(bytes) = byte_stream.next().await {
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
                                // 部分提供商会混入非标准行，跳过即可
                                tracing::debug!("skip non-standard SSE line: {} — {}", e, data);
                            }
                        }
                    }
                }
            }
        }

        // 处理末尾残留数据（某些服务不以 \n\n 结尾）
        for line in buffer.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if data.trim() != "[DONE]" {
                    if let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) {
                        yield chunk;
                    }
                }
            }
        }
    };

    Ok(stream)
}
