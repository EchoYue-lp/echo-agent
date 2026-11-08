// D1字段声明前禁止运行：直接使用Root指定的DeltaMessage.provider_response，
// 类型由Frame唯一决定；本文件不声明/镜像metadata类型，不要求DeltaMessage Serialize。
#[tokio::test]
async fn chat_metadata_d1_stream_raw_usage_snapshot_and_done_barrier() -> Result<()> {
    let fixture = chat_metadata_fixture()?;
    for case in fixture
        .get("stream")
        .and_then(Value::as_array)
        .ok_or(LlmError::EmptyResponse)?
    {
        let events = case
            .get("events")
            .and_then(Value::as_array)
            .ok_or(LlmError::EmptyResponse)?;
        let provider = case
            .get("provider")
            .and_then(Value::as_str)
            .ok_or(LlmError::EmptyResponse)?;
        let model = case
            .get("model")
            .and_then(Value::as_str)
            .ok_or(LlmError::EmptyResponse)?;
        let (url, handle) = compat_server(vec![(
            200,
            "text/event-stream".into(),
            compat_sse(events, true),
        )])
        .await?;
        let client = OpenAiClient::new(LlmConfig::for_provider(
            provider,
            &url,
            "fixture-only",
            model,
            LlmApiProtocol::ChatCompletions,
        )?)?;
        let items = client
            .chat_stream(ChatRequest::new(vec![Message::user("snapshots".into())]))
            .await?
            .collect::<Vec<_>>()
            .await;
        let chunks = items.into_iter().collect::<Result<Vec<_>>>()?;
        assert_eq!(
            chunks
                .iter()
                .filter(|chunk| chunk.finish_reason.is_some())
                .count(),
            1
        );
        let last = chunks.last().ok_or(LlmError::EmptyResponse)?;
        assert_eq!(last.finish_reason.as_deref(), Some("stop"));
        for chunk in chunks.iter().take(chunks.len().saturating_sub(1)) {
            assert!(chunk.finish_reason.is_none());
            assert!(
                chunk.usage.is_none(),
                "normalized usage must remain behind DONE barrier"
            );
        }
        let snapshots = chunks
            .iter()
            .filter_map(|chunk| chunk.delta.provider_response.as_ref())
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| LlmError::SerializationError(error.to_string()))?;
        let metadata = snapshots.last().ok_or(LlmError::EmptyResponse)?;
        assert_native_metadata(
            metadata,
            provider,
            "stream-native",
            &json!({"model":case.get("expected_model").ok_or(LlmError::EmptyResponse)?,"usage":case.get("expected_usage").ok_or(LlmError::EmptyResponse)?,"stop_reason":"stop"}),
        )?;
        for snapshot in &snapshots {
            assert_eq!(
                snapshot.get("provider").and_then(Value::as_str),
                Some(provider)
            );
            assert_eq!(
                snapshot.get("request_id").and_then(Value::as_str),
                Some("stream-native")
            );
            // 完整观察快照：后来的usage-only更新不能丢掉此前实际model。
            assert_eq!(snapshot.get("model"), case.get("expected_model"));
        }
        let normalized = last.usage.as_ref().ok_or(LlmError::EmptyResponse)?;
        assert_eq!(
            normalized.total_tokens,
            case.pointer("/expected_usage/total_tokens")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
        );
        let _captured = compat_captured(handle).await?;
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d1_cross_request_id_change_is_never_success() -> Result<()> {
    let fixture = chat_metadata_fixture()?;
    let events = fixture
        .get("changed_id_stream")
        .and_then(Value::as_array)
        .ok_or(LlmError::EmptyResponse)?;
    let (url, handle) = compat_server(vec![(
        200,
        "text/event-stream".into(),
        compat_sse(events, true),
    )])
    .await?;
    let items = compat_client(&url)?
        .chat_stream(ChatRequest::new(vec![Message::user(
            "request scope".into(),
        )]))
        .await?
        .collect::<Vec<_>>()
        .await;
    assert!(items.iter().any(Result::is_err));
    for chunk in items.iter().filter_map(|item| item.as_ref().ok()) {
        assert!(chunk.finish_reason.is_none() && chunk.usage.is_none());
        if let Some(observation) = chunk.delta.provider_response.as_ref() {
            let value = serde_json::to_value(observation)
                .map_err(|error| LlmError::SerializationError(error.to_string()))?;
            assert_eq!(
                value.get("request_id").and_then(Value::as_str),
                Some("request-A")
            );
        }
    }
    let _captured = compat_captured(handle).await?;
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d1_frame_observation_only_filter_preserves_real_empty_tool_message()
-> Result<()> {
    let fixture = chat_metadata_fixture()?;
    let wire = fixture
        .pointer("/nonstream/0/wire")
        .ok_or(LlmError::EmptyResponse)?
        .to_string();
    let (url, handle) = compat_server(vec![
        (200, "application/json".into(), wire.clone()),
        (200, "application/json".into(), wire),
    ])
    .await?;
    let client = compat_client(&url)?;
    let first = client
        .chat(ChatRequest::new(vec![Message::user("first".into())]))
        .await?;
    let observation = first
        .message
        .provider_response
        .clone()
        .ok_or(LlmError::EmptyResponse)?;
    let observation_only = Message::provider_response_observation(observation.clone());
    assert!(observation_only.is_provider_response_observation_only());
    let mut legitimate:Message=serde_json::from_value(json!({"role":"assistant","content":null,"tool_calls":[{"id":"real-empty-tool","type":"function","function":{"name":"lookup","arguments":"{}"}}]})).map_err(|error|LlmError::SerializationError(error.to_string()))?;
    legitimate.provider_response = Some(observation);
    assert!(!legitimate.is_provider_response_observation_only());
    let _reply = client
        .chat(ChatRequest::new(vec![
            Message::user("first".into()),
            observation_only,
            legitimate,
            Message::tool_result("real-empty-tool".into(), "lookup".into(), "ok".into()),
            Message::user("next".into()),
        ]))
        .await?;
    let captured = compat_captured(handle).await?;
    let request = captured.get(1).ok_or(LlmError::EmptyResponse)?;
    assert_eq!(
        request
            .get("messages")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(4)
    );
    assert_eq!(
        request
            .pointer("/messages/1/tool_calls/0/id")
            .and_then(Value::as_str),
        Some("real-empty-tool")
    );
    assert!(!request.to_string().contains("provider_response"));
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d1_usage_only_raw_u64_null_is_observed_before_done() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let expected_usage = json!({"prompt_tokens":1,"completion_tokens":2,"total_tokens":3,"native_extra":{"max_u64":u64::MAX,"explicit_null":null}});
    let usage = expected_usage.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let captured = resume_read_http_body(&mut socket).await?;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await?;
        let before_done = compat_sse(
            &[
                json!({"id":"before-done","model":"native-model","choices":[{"index":0,"delta":{"content":"body"},"finish_reason":null}]}),
                json!({"id":"before-done","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
                json!({"id":"before-done","choices":[],"usage":usage}),
            ],
            false,
        );
        socket.write_all(before_done.as_bytes()).await?;
        socket.flush().await?;
        release_rx
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))?;
        socket.write_all(b"data: [DONE]\n\n").await?;
        socket.shutdown().await?;
        Ok::<Value, ReactError>(captured)
    });
    let client = compat_client(&url)?;
    let mut stream = client
        .chat_stream(ChatRequest::new(vec![Message::user("before DONE".into())]))
        .await?;
    let observed = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(item) = stream.next().await {
            let chunk = item?;
            assert!(chunk.finish_reason.is_none() && chunk.usage.is_none());
            if let Some(record) = chunk.delta.provider_response.as_ref() {
                let value = serde_json::to_value(record)
                    .map_err(|error| LlmError::SerializationError(error.to_string()))?;
                if value.get("usage") == Some(&expected_usage) {
                    return Ok::<Value, ReactError>(value);
                }
            }
        }
        Err(LlmError::InvalidResponse("usage-only raw observation waited for DONE".into()).into())
    })
    .await
    .map_err(|error| LlmError::NetworkError(error.to_string()))??;
    assert_eq!(
        observed
            .pointer("/usage/native_extra/max_u64")
            .and_then(Value::as_u64),
        Some(u64::MAX)
    );
    assert_eq!(
        observed.pointer("/usage/native_extra/explicit_null"),
        Some(&Value::Null)
    );
    assert_eq!(
        observed.get("model").and_then(Value::as_str),
        Some("native-model")
    );
    assert_eq!(
        observed.get("stop_reason").and_then(Value::as_str),
        Some("stop")
    );
    release_tx
        .send(())
        .map_err(|_| LlmError::NetworkError("DONE gate closed".into()))?;
    let rest = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(
        rest.iter()
            .filter(|chunk| chunk.finish_reason.is_some())
            .count(),
        1
    );
    assert_eq!(
        rest.last()
            .and_then(|chunk| chunk.usage.as_ref())
            .and_then(|usage| usage.total_tokens),
        Some(3)
    );
    let _captured = server
        .await
        .map_err(|error| LlmError::NetworkError(error.to_string()))??;
    Ok(())
}

#[test]
fn chat_metadata_d1_common_filter_never_drops_empty_native_reasoning_message() -> Result<()> {
    use echo_core::llm::types::ReasoningBlock;
    let observation: echo_core::llm::types::ProviderResponseMetadata = serde_json::from_value(
        json!({"protocol":"chat_completions","provider":"test","request_id":"native"}),
    )
    .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    for reasoning in [
        ReasoningBlock::Signed {
            thinking: "signed-original".into(),
            signature: "sig".into(),
        },
        ReasoningBlock::Redacted {
            data: "redacted-original".into(),
        },
        ReasoningBlock::Opaque {
            provider: "responses".into(),
            id: "reason-item".into(),
            data: "opaque-original".into(),
            summary: vec![],
        },
    ] {
        let mut message = Message::provider_response_observation(observation.clone());
        message.reasoning_blocks = Some(vec![reasoning]);
        assert!(!message.is_provider_response_observation_only());
        // 这里只验metadata-only整条过滤资格。Chat继续沿原规则不发送其它协议
        // signed/opaque wire，但不得把canonical reasoning消息误当失败空记录删掉。
        let serialized_before = serde_json::to_value(&message)
            .map_err(|error| LlmError::SerializationError(error.to_string()))?;
        let normalized = normalize_messages(vec![message.clone()])?;
        assert_eq!(normalized.len(), 1);
        assert!(message.reasoning_blocks.is_some());
        assert_eq!(
            serde_json::to_value(&message)
                .map_err(|error| LlmError::SerializationError(error.to_string()))?,
            serialized_before
        );
    }
    Ok(())
}
