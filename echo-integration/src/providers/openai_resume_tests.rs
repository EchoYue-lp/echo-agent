// 恢复组合专属回归：复用既有 HTTP helper 与真实 OpenAiClient，未执行不作通过证据。

#[tokio::test]
async fn chat_resume_typed_sse_error_before_after_finish_preserves_all_false() -> Result<()> {
    use echo_core::llm::ThinkingConfig;
    for (provider, model) in [
        ("openai", "gpt-4o-2024-08-06"),
        ("deepseek", "deepseek-v4-pro"),
        ("custom", "custom-chat"),
    ] {
        for after_finish in [false, true] {
            for code in [
                None,
                Some(Value::Null),
                Some(json!(37)),
                Some(json!("rate_limit_error")),
            ] {
                let mut payload =
                    json!({"type":"server_error","message":"429 overloaded 原服务🙂"});
                if let Some(code) = code.as_ref() {
                    payload
                        .as_object_mut()
                        .ok_or(LlmError::EmptyResponse)?
                        .insert("code".into(), code.clone());
                }
                let progress = json!({"id":"typed-composed","choices":[{"index":0,"delta":{"reasoning_content":"先核对🙂","content":"正在查询","tool_calls":[{"index":0,"id":"tool-original","type":"function","function":{"name":"lookup","arguments":"{}"}}]},"finish_reason":null}]});
                let finish = json!({"id":"typed-composed","choices":[{"index":0,"delta":null,"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":11,"completion_tokens":13,"total_tokens":24,"completion_tokens_details":{"reasoning_tokens":9}}});
                let error = json!({"error":payload});
                let events = if after_finish {
                    vec![progress, finish, error]
                } else {
                    vec![progress, error, finish]
                };
                let (url, handle) = compat_server(vec![(
                    200,
                    "text/event-stream".into(),
                    compat_sse(&events, true),
                )])
                .await?;
                let client = OpenAiClient::new(LlmConfig::for_provider(
                    provider,
                    &url,
                    "fixture-only",
                    model,
                    LlmApiProtocol::ChatCompletions,
                )?)?;
                let mut request = compat_request(
                    ToolChoice::function("lookup"),
                    (provider == "openai").then_some(true),
                );
                request.response_format = None;
                if provider == "deepseek" {
                    request.thinking = Some(ThinkingConfig::Disabled);
                }
                let items = client.chat_stream(request).await?.collect::<Vec<_>>().await;
                let error = items
                    .iter()
                    .find_map(|item| item.as_ref().err())
                    .ok_or_else(|| {
                        LlmError::InvalidResponse("composed transport swallowed typed error".into())
                    })?;
                assert!(
                    matches!(error, ReactError::Llm(error) if matches!(error.as_ref(), LlmError::ProviderError {
                    provider: actual_provider, error_type, code: actual_code, message, retryable:false
                } if actual_provider == provider && error_type.as_deref() == Some("server_error") && actual_code == &code && message == "429 overloaded 原服务🙂"))
                );
                let failure = echo_core::error::AgentFailure::try_from(error)?;
                assert_eq!(failure.code, "llm_provider");
                assert!(!failure.retryable);
                assert_eq!(failure.http_status, None);
                for chunk in items.iter().filter_map(|item| item.as_ref().ok()) {
                    assert!(chunk.finish_reason.is_none() && chunk.usage.is_none());
                }
                let reasoning = items
                    .iter()
                    .filter_map(|item| item.as_ref().ok())
                    .filter_map(|chunk| chunk.delta.reasoning_content.as_deref())
                    .collect::<String>();
                assert_eq!(reasoning, "先核对🙂");
                let captured = compat_captured(handle).await?;
                let request = captured.first().ok_or(LlmError::EmptyResponse)?;
                assert_eq!(
                    request.get("tool_choice"),
                    Some(&ToolChoice::function("lookup").to_openai_value())
                );
                assert_eq!(
                    request
                        .pointer("/tools/0/function/strict")
                        .and_then(Value::as_bool),
                    (provider == "openai").then_some(true)
                );
            }
        }
    }
    Ok(())
}

async fn resume_read_http_body(socket: &mut tokio::net::TcpStream) -> Result<Value> {
    let mut bytes = Vec::new();
    loop {
        let count = tokio::time::timeout(Duration::from_secs(2), socket.read_buf(&mut bytes))
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))??;
        if count == 0 || bytes.len() > 1_048_576 {
            return Err(
                LlmError::InvalidResponse("bounded fixture HTTP body missing".into()).into(),
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
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                })
                .ok_or(LlmError::EmptyResponse)?;
            if payload.len() >= length {
                return serde_json::from_str(payload)
                    .map_err(|error| LlmError::InvalidResponse(error.to_string()).into());
            }
        }
    }
}

#[tokio::test]
async fn chat_resume_pending_finish_waits_done_and_honors_late_error_eof_cancel() -> Result<()> {
    for ending in ["done", "provider_error", "eof", "cancel"] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let (written_tx, written_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let token = tokio_util::sync::CancellationToken::new();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let request = resume_read_http_body(&mut socket).await?;
            // 只完成一个provider响应；无第二模型请求、无额外执行循环。
            let prefix = compat_sse(
                &[
                    json!({"id":"gated-chat","choices":[{"index":0,"delta":{"reasoning_content":"原推理","content":"已输出正文"},"finish_reason":null}]}),
                    json!({"id":"gated-chat","choices":[{"index":0,"delta":null,"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7,"completion_tokens_details":{"reasoning_tokens":2}}}),
                ],
                false,
            );
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await?;
            socket.write_all(prefix.as_bytes()).await?;
            socket.flush().await?;
            written_tx
                .send(())
                .map_err(|_| LlmError::NetworkError("test observer closed".into()))?;
            release_rx
                .await
                .map_err(|error| LlmError::NetworkError(error.to_string()))?;
            match ending {
                "done" => socket.write_all(b"data: [DONE]\n\n").await?,
                "provider_error" => socket.write_all(compat_sse(&[json!({"error":{"type":"overloaded_error","message":"late upstream failure"}})],true).as_bytes()).await?,
                _ => {}
            }
            socket.shutdown().await?;
            Ok::<Value, ReactError>(request)
        });
        let client = compat_client(&url)?;
        let mut request = ChatRequest::new(vec![Message::user("不要猜阶段".into())]);
        request.cancel_token = Some(token.clone());
        let mut stream = client.chat_stream(request).await?;
        let progress = stream.next().await.ok_or(LlmError::EmptyResponse)??;
        assert_eq!(progress.delta.reasoning_content.as_deref(), Some("原推理"));
        assert_eq!(progress.delta.content.as_deref(), Some("已输出正文"));
        assert!(progress.finish_reason.is_none() && progress.usage.is_none());
        tokio::time::timeout(Duration::from_secs(2), written_rx)
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))?
            .map_err(|error| LlmError::NetworkError(error.to_string()))?;
        loop {
            match tokio::time::timeout(Duration::from_millis(30), stream.next()).await {
                Err(_) => break,
                Ok(Some(Ok(chunk))) => {
                    assert!(
                        chunk.finish_reason.is_none() && chunk.usage.is_none(),
                        "finish/usage emitted before DONE"
                    );
                    assert!(chunk.delta.provider_response.is_some());
                    assert!(chunk.delta.content.is_none() && chunk.delta.tool_calls.is_none());
                }
                Ok(Some(Err(error))) => return Err(error),
                Ok(None) => {
                    return Err(LlmError::InvalidResponse(
                        "stream ended before fixture gate".into(),
                    )
                    .into());
                }
            }
        }
        if ending == "cancel" {
            token.cancel();
        }
        release_tx
            .send(())
            .map_err(|_| LlmError::NetworkError("fixture gate closed".into()))?;
        let rest = tokio::time::timeout(Duration::from_secs(2), stream.collect::<Vec<_>>())
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))?;
        if ending == "done" {
            let chunks = rest.into_iter().collect::<Result<Vec<_>>>()?;
            let terminal = chunks
                .iter()
                .filter(|chunk| chunk.finish_reason.is_some())
                .collect::<Vec<_>>();
            assert_eq!(terminal.len(), 1);
            let usage = terminal
                .first()
                .and_then(|chunk| chunk.usage.as_ref())
                .ok_or(LlmError::EmptyResponse)?;
            assert_eq!(usage.total_tokens, Some(7));
            assert_eq!(
                usage
                    .output_tokens_details
                    .as_ref()
                    .and_then(|detail| detail.reasoning_tokens),
                Some(2)
            );
        } else {
            let error = rest
                .iter()
                .find_map(|item| item.as_ref().err())
                .ok_or(LlmError::EmptyResponse)?;
            if ending == "provider_error" {
                assert!(
                    matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::ProviderError{retryable:false,..}))
                );
            }
            if ending == "cancel" {
                assert!(
                    matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::NetworkError(message) if message.contains("cancelled")))
                );
            }
            for chunk in rest.iter().filter_map(|item| item.as_ref().ok()) {
                assert!(chunk.finish_reason.is_none() && chunk.usage.is_none());
            }
        }
        let _captured = server
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))??;
    }
    Ok(())
}
