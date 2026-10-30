//! F684 已有 API 与 GREEN-v02 ProviderError API 分组回归；全部是待执行定义。

use super::*;
use serde_json::{Value, json};

fn invalid(reason: &str) -> LlmError {
    LlmError::InvalidResponse(reason.into())
}

#[test]
fn old_api_signed_redacted_tool_and_text_cold_replay_keep_original_order() -> Result<()> {
    let raw = json!({"id":"resume-original","content":[
        {"type":"text","text":"public before"},
        {"type":"thinking","thinking":"signed 中文😀","signature":"sig-original=="},
        {"type":"tool_use","id":"resume-tool","name":"lookup","input":{"path":"中文.md"}},
        {"type":"redacted_thinking","data":"redacted-original=="},
        {"type":"text","text":"public after"}
    ],"stop_reason":"tool_use"});
    let client = AnthropicClient::new("fixture-only", "test-model");
    let response = client.convert_response(
        serde_json::from_value(raw.clone())
            .map_err(|error| LlmError::InvalidResponse(error.to_string()))?,
    )?;
    let bytes = serde_json::to_vec(&response.message)
        .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    let restored: Message = serde_json::from_slice(&bytes)
        .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
    let request = client.convert_request(&ChatRequest::new(vec![restored]))?;
    let wire = serde_json::to_value(request)
        .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    let blocks = wire
        .pointer("/messages/0/content")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("native replay blocks missing"))?;
    let expected = raw
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("fixture content missing"))?;
    assert_eq!(blocks.len(), expected.len());
    for (block, expected) in blocks.iter().zip(expected) {
        let mut block = block.clone();
        if let Some(object) = block.as_object_mut() {
            object.remove("cache_control");
        }
        assert_eq!(&block, expected);
    }
    Ok(())
}

#[test]
fn old_api_pause_reasoning_is_continue_and_empty_redacted_is_rejected() -> Result<()> {
    let client = AnthropicClient::new("fixture-only", "test-model");
    for block in [
        json!({"type":"thinking","thinking":"continue","signature":"sig"}),
        json!({"type":"redacted_thinking","data":"opaque-original"}),
    ] {
        let raw = json!({"id":"pause","content":[block],"stop_reason":"pause_turn"});
        let response = client.convert_response(
            serde_json::from_value(raw)
                .map_err(|error| LlmError::InvalidResponse(error.to_string()))?,
        )?;
        assert_eq!(response.finish_reason.as_deref(), Some("continue"));
        assert!(response.message.content.as_text().is_none());
    }
    let raw = json!({"id":"bad-redacted","content":[{"type":"redacted_thinking","data":""},{"type":"text","text":"visible"}],"stop_reason":"end_turn"});
    assert!(
        client
            .convert_response(
                serde_json::from_value(raw)
                    .map_err(|error| LlmError::InvalidResponse(error.to_string()))?
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn old_api_stop_classification_does_not_promote_pause_or_truncation() -> Result<()> {
    assert_eq!(
        anthropic_finish_reason(Some("pause_turn"))?.as_deref(),
        Some("continue")
    );
    for stop in ["end_turn", "stop_sequence", "refusal"] {
        assert_eq!(
            anthropic_finish_reason(Some(stop))?.as_deref(),
            Some("stop")
        );
    }
    for stop in [
        Some("max_tokens"),
        Some("model_context_window_exceeded"),
        Some("unknown"),
        None,
    ] {
        assert!(anthropic_finish_reason(stop).is_err());
    }
    Ok(())
}

#[test]
fn old_api_usage_updates_preserve_existing_cache_counts_and_reject_overflow() -> Result<()> {
    let mut adapter = NativeAnthropicStream::default();
    adapter.map(json!({"type":"message_start","message":{"id":"usage-native","usage":{"input_tokens":11,"output_tokens":0,"cache_creation_input_tokens":2,"cache_read_input_tokens":3}}}))?;
    adapter.map(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}))?;
    assert_eq!(adapter.usage.prompt_tokens, Some(11));
    assert_eq!(adapter.usage.completion_tokens, Some(7));
    assert_eq!(adapter.usage.cache_creation_input_tokens, Some(2));
    assert_eq!(adapter.usage.cache_read_input_tokens, Some(3));
    assert_eq!(adapter.usage.effective_total_tokens(), 23);
    let mut invalid_adapter = NativeAnthropicStream::default();
    assert!(invalid_adapter.map(json!({"type":"message_start","message":{"id":"overflow","usage":{"input_tokens":4294967296_u64,"output_tokens":0}}})).is_err());
    Ok(())
}

#[tokio::test]
async fn old_api_http_raw_retains_original_absent_null_and_extension_fields() -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    for stop_sequence in [None, Some(Value::Null), Some(json!("STOP-original"))] {
        let mut raw = json!({"id":"raw-original","model":"vendor-model-original","content":[{"type":"text","text":"visible","citations":[{"vendor":"citation-original"}]}],"stop_reason":"stop_sequence","usage":{"input_tokens":2,"output_tokens":3,"service_tier":"original","nested":{"tokens":11}},"vendor_extension":{"request":"original"}});
        if let Some(sequence) = stop_sequence {
            raw.as_object_mut()
                .ok_or_else(|| invalid("raw fixture object missing"))?
                .insert("stop_sequence".into(), sequence);
        }
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let body = raw.to_string();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut bytes = Vec::new();
            loop {
                if socket.read_buf(&mut bytes).await? == 0 {
                    return Err(invalid("request closed before full body").into());
                }
                let text = match std::str::from_utf8(&bytes) {
                    Ok(text) => text,
                    Err(error) if error.error_len().is_none() => continue,
                    Err(_) => return Err(invalid("fixture request invalid UTF-8").into()),
                };
                if let Some((headers, payload)) = text.split_once("\r\n\r\n") {
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        })
                        .ok_or_else(|| invalid("content length missing"))?;
                    if payload.len() >= length {
                        break;
                    }
                }
            }
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
            socket.flush().await?;
            Ok::<_, echo_core::error::ReactError>(())
        });
        let client = AnthropicClient::with_base_url(
            format!("http://{address}"),
            "fixture-only",
            "test-model",
        );
        let response = client
            .chat(ChatRequest::new(vec![Message::user("request".into())]))
            .await;
        if response.is_err() {
            server.abort();
            let _settled = server.await;
        } else {
            server
                .await
                .map_err(|error| LlmError::InvalidResponse(error.to_string()))??;
        }
        let response = response?;
        assert_eq!(response.raw.extra.as_ref(), Some(&raw));
        assert_eq!(response.raw.model.as_deref(), Some("vendor-model-original"));
    }
    Ok(())
}

// 以下依赖 GREEN-v02 已有 ProviderError 与 reported_provider_error；不能在原 F684 冒作旧API RED。
#[test]
fn selected_api_provider_error_preserves_all_code_types_and_never_grants_retry() -> Result<()> {
    let codes = [
        None,
        Some(Value::Null),
        Some(json!(429)),
        Some(json!("rate_limited")),
        Some(json!({"nested":["中文",7]})),
    ];
    for error_type in [Some("overloaded_error"), Some("unknown_future_error"), None] {
        for code in &codes {
            let mut payload = json!({"message":"原始服务错误😀"});
            let object = payload
                .as_object_mut()
                .ok_or_else(|| invalid("error fixture object missing"))?;
            if let Some(kind) = error_type {
                object.insert("type".into(), json!(kind));
            }
            if let Some(code) = code {
                object.insert("code".into(), code.clone());
            }
            let mut adapter = NativeAnthropicStream {
                provider: "configured-service".into(),
                ..Default::default()
            };
            let error = adapter
                .map(json!({"type":"error","error":payload}))
                .err()
                .ok_or_else(|| invalid("provider error was accepted"))?;
            assert!(matches!(&error, echo_core::error::ReactError::Llm(inner)
                if matches!(inner.as_ref(), LlmError::ProviderError { provider, error_type: kind, code: actual, message, retryable: false }
                    if provider == "configured-service" && kind.as_deref() == error_type && actual == code && message == "原始服务错误😀")));
            let failure = echo_core::error::AgentFailure::try_from(&error)?;
            assert_eq!(failure.http_status, None);
            assert_eq!(failure.code, "llm_provider");
            assert!(!failure.retryable);
        }
    }
    Ok(())
}

#[test]
fn selected_api_error_after_message_delta_has_no_success_terminal() -> Result<()> {
    let mut adapter = NativeAnthropicStream {
        provider: "configured-service".into(),
        ..Default::default()
    };
    let events = [
        json!({"type":"message_start","message":{"id":"request-original","usage":{"input_tokens":2,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"signed original"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-original"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":"partial visible"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}),
    ];
    let mut seen = Vec::new();
    for event in events {
        seen.extend(adapter.map(event)?);
    }
    assert!(seen.iter().all(|chunk| chunk.finish_reason.is_none()));
    assert!(seen.iter().flat_map(|chunk| chunk.delta.assistant_blocks.iter().flatten()).any(|block| matches!(&block.reasoning, Some(ReasoningBlock::Signed { thinking, signature }) if thinking == "signed original" && signature == "sig-original")));
    let error = adapter.map(json!({"type":"error","error":{"type":"overloaded_error","code":529,"message":"busy original"}})).err().ok_or_else(|| invalid("late provider error omitted"))?;
    assert!(
        matches!(error, echo_core::error::ReactError::Llm(inner) if matches!(inner.as_ref(), LlmError::ProviderError { retryable:false, .. }))
    );
    Ok(())
}
