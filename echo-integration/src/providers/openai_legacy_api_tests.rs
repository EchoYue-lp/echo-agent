// 只用 GREEN-v02 已有 Rust API 和 serde 输入；不直接引用 strict/错误位新字段，
// 不命名新的请求 ToolChoice 类型。新旧实现均可编译；尚未实际执行语义 RED。
use super::*;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn legacy_http_fixture() -> Result<(OpenAiClient, tokio::task::JoinHandle<Result<Value>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))??;
        let mut bytes = Vec::new();
        let request = loop {
            let count = socket.read_buf(&mut bytes).await?;
            if count == 0 || bytes.len() > 1_048_576 {
                return Err(
                    LlmError::InvalidResponse("legacy fixture body truncated".into()).into(),
                );
            }
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            if let Some((headers, payload)) = text.split_once("\r\n\r\n") {
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    })
                    .ok_or(LlmError::EmptyResponse)?;
                if payload.len() >= length {
                    break serde_json::from_str::<Value>(payload)
                        .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
                }
            }
        };
        let response=json!({"id":"legacy-fixture","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}).to_string();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response.len()).as_bytes()).await?;
        socket.write_all(response.as_bytes()).await?;
        socket.shutdown().await?;
        Ok(request)
    });
    let config = LlmConfig::for_provider(
        "openai",
        &url,
        "fixture-only",
        "gpt-4o-2024-08-06",
        echo_core::llm::LlmApiProtocol::ChatCompletions,
    )?
    .with_input_modalities(echo_core::llm::ModelInputModality::all_supported());
    Ok((OpenAiClient::new(config)?, server))
}

#[test]
fn chat_legacy_api_selected_function_object_roundtrip() -> Result<()> {
    // 原 ChatCompletionRequest 已有 Deserialize/Serialize。旧String字段会拒绝
    // 合法function对象；这是运行期serde行为失败，不能混成新API编译失败。
    let wire = json!({"model":"gpt-4o-2024-08-06","messages":[{"role":"user","content":"选择函数"}],"tool_choice":{"type":"function","function":{"name":"lookup"}}});
    let request: echo_core::llm::types::ChatCompletionRequest =
        serde_json::from_value(wire.clone())
            .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    let rebuilt = serde_json::to_value(request)
        .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    assert_eq!(rebuilt.get("tool_choice"), wire.get("tool_choice"));
    Ok(())
}

#[tokio::test]
async fn chat_legacy_api_function_strict_serde_reaches_actual_http() -> Result<()> {
    for strict in [false, true] {
        let (client, server) = legacy_http_fixture().await?;
        let tool: echo_core::llm::types::ToolDefinition=serde_json::from_value(json!({"type":"function","function":{"name":"lookup","description":"test","parameters":{"type":"object","properties":{},"additionalProperties":false},"strict":strict}})).map_err(|error|LlmError::SerializationError(error.to_string()))?;
        let request =
            ChatRequest::new(vec![Message::user("JSON工具".into())]).with_tools(vec![tool]);
        let _reply = client.chat(request).await?;
        let captured = server
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))??;
        assert_eq!(
            captured
                .pointer("/tools/0/function/strict")
                .and_then(Value::as_bool),
            Some(strict)
        );
    }
    Ok(())
}

#[test]
fn chat_legacy_api_tool_error_observation_serde_retains_false_and_true() -> Result<()> {
    for is_error in [false, true] {
        let wire = json!({"role":"tool","tool_call_id":"original","name":"lookup","content":"原文🙂","tool_result_is_error":is_error});
        let message: Message = serde_json::from_value(wire)
            .map_err(|error| LlmError::SerializationError(error.to_string()))?;
        let saved = serde_json::to_value(message)
            .map_err(|error| LlmError::SerializationError(error.to_string()))?;
        assert_eq!(
            saved.get("tool_result_is_error").and_then(Value::as_bool),
            Some(is_error)
        );
    }
    Ok(())
}

#[tokio::test]
async fn chat_legacy_api_direct_tool_image_is_not_silently_accepted() -> Result<()> {
    let (client, server) = legacy_http_fixture().await?;
    let tool: Message=serde_json::from_value(json!({"role":"tool","tool_call_id":"original","name":"lookup","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,aW1n"}}]})).map_err(|error|LlmError::SerializationError(error.to_string()))?;
    let result = client.chat(ChatRequest::new(vec![tool])).await;
    // 新行为须在发送前明确拒绝；旧行为会真的发送不支持的tool图片并获fixture 200。
    if let Err(error) = &result {
        server.abort();
        let _settled = server.await;
        assert!(error.to_string().contains("supports text parts only"));
    } else {
        let _captured = server
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))??;
        assert!(
            result.is_err(),
            "unsupported OpenAI tool image was silently accepted"
        );
    }
    Ok(())
}
