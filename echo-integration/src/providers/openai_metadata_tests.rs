// D1 caller反控：只用现有Message serde与真实OpenAiClient，不声明metadata类型。
fn chat_metadata_fixture() -> Result<Value> {
    serde_json::from_str(include_str!(
        "../../fixtures/providers/chat/metadata-r01.json"
    ))
    .map_err(|error| LlmError::SerializationError(error.to_string()).into())
}

fn chat_metadata_value(message: &Message) -> Result<Value> {
    serde_json::to_value(message)
        .map_err(|error| LlmError::SerializationError(error.to_string()))?
        .get("provider_response")
        .cloned()
        .filter(Value::is_object)
        .ok_or_else(|| {
            LlmError::InvalidResponse("canonical Message omitted provider_response".into()).into()
        })
}

fn assert_native_metadata(
    metadata: &Value,
    provider: &str,
    rid: &str,
    expected: &Value,
) -> Result<()> {
    assert_eq!(
        metadata.get("protocol").and_then(Value::as_str),
        Some("chat_completions")
    );
    assert_eq!(
        metadata.get("provider").and_then(Value::as_str),
        Some(provider)
    );
    assert_eq!(
        metadata.get("request_id").and_then(Value::as_str),
        Some(rid)
    );
    for key in ["model", "usage", "stop_reason"] {
        assert_eq!(
            metadata.get(key),
            expected.get(key),
            "native field membership/value differs for {key}"
        );
    }
    // Chat没有这三个其它协议字段，不从stop/usage推造。
    for key in ["stop_sequence", "status", "incomplete_details"] {
        assert!(metadata.get(key).is_none());
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d1_nonstream_missing_null_nested_and_spoof() -> Result<()> {
    let fixture = chat_metadata_fixture()?;
    for provider in ["openai", "deepseek", "bailian"] {
        for case in fixture
            .get("nonstream")
            .and_then(Value::as_array)
            .ok_or(LlmError::EmptyResponse)?
        {
            let wire = case.get("wire").ok_or(LlmError::EmptyResponse)?;
            let (url, handle) =
                compat_server(vec![(200, "application/json".into(), wire.to_string())]).await?;
            let model = match provider {
                "deepseek" => "deepseek-v4-pro",
                "bailian" => "qwen3.8-max",
                _ => "gpt-4o-2024-08-06",
            };
            let client = OpenAiClient::new(LlmConfig::for_provider(
                provider,
                &url,
                "fixture-only",
                model,
                LlmApiProtocol::ChatCompletions,
            )?)?;
            let response = client
                .chat(ChatRequest::new(vec![Message::user("metadata".into())]))
                .await?;
            let metadata = chat_metadata_value(&response.message)?;
            assert_native_metadata(
                &metadata,
                provider,
                "native-meta-1",
                case.get("expected_native_observation")
                    .ok_or(LlmError::EmptyResponse)?,
            )?;
            assert!(!metadata.to_string().contains("wire-spoof"));
            assert!(response.message.execution_status.is_none());
            assert!(response.message.approved_assistant_blocks.is_none());
            assert_eq!(
                response.message.reasoning_content.as_deref(),
                Some("原生推理")
            );
            let _captured = compat_captured(handle).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d1_cold_history_is_local_only_and_next_request_is_isolated() -> Result<()> {
    let fixture = chat_metadata_fixture()?;
    let cases = fixture
        .get("cross_request")
        .and_then(Value::as_array)
        .ok_or(LlmError::EmptyResponse)?;
    let first_wire = cases.first().ok_or(LlmError::EmptyResponse)?;
    let second_wire = cases.get(1).ok_or(LlmError::EmptyResponse)?;
    let (url, handle) = compat_server(vec![
        (200, "application/json".into(), first_wire.to_string()),
        (200, "application/json".into(), second_wire.to_string()),
    ])
    .await?;
    let client = OpenAiClient::new(LlmConfig::for_provider(
        "deepseek",
        &url,
        "fixture-only",
        "deepseek-v4-pro",
        LlmApiProtocol::ChatCompletions,
    )?)?;
    let first = client
        .chat(ChatRequest::new(vec![Message::user("first".into())]))
        .await?;
    let metadata = chat_metadata_value(&first.message)?;
    let bytes = serde_json::to_vec(&first.message)
        .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    let restored: Message = serde_json::from_slice(&bytes)
        .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    assert_eq!(chat_metadata_value(&restored)?, metadata);
    let second = client
        .chat(ChatRequest::new(vec![
            Message::user("first".into()),
            restored,
            Message::user("next".into()),
        ]))
        .await?;
    let observation = chat_metadata_value(&second.message)?;
    assert_native_metadata(
        &observation,
        "deepseek",
        "request-B",
        &json!({"stop_reason":"stop"}),
    )?;
    assert!(!observation.to_string().contains("A-only"));
    let requests = compat_captured(handle).await?;
    let second_request = requests.get(1).ok_or(LlmError::EmptyResponse)?;
    assert!(!second_request.to_string().contains("provider_response"));
    assert_eq!(
        second_request
            .pointer("/messages/1/reasoning_content")
            .and_then(Value::as_str),
        Some("原生推理")
    );
    Ok(())
}
