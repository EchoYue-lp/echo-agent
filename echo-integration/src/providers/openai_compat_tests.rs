// 本文件由 openai.rs 的 tests 模块 include；真实 HTTP 捕获而非调用 request_body 替代生产请求。
use echo_core::llm::types::{FunctionSpec, ResponseFormat, ToolDefinition};
use echo_core::llm::{LlmApiProtocol, ModelInputModality, ToolChoice};
use futures::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn compat_fixture() -> Result<Value> {
    serde_json::from_str(include_str!(
        "../../fixtures/providers/chat/compatibility-roundtrip.json"
    ))
    .map_err(|error| LlmError::InvalidResponse(error.to_string()).into())
}

fn compat_client(url: &str) -> Result<OpenAiClient> {
    OpenAiClient::new(
        LlmConfig::for_provider(
            "openai",
            url,
            "fixture-only",
            "gpt-4o-2024-08-06",
            LlmApiProtocol::ChatCompletions,
        )?
        .with_input_modalities(vec![ModelInputModality::Text, ModelInputModality::Image]),
    )
}

fn compat_tool(strict: Option<bool>) -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".into(),
        function: FunctionSpec {
            name: "lookup".into(),
            description: "查天气".into(),
            parameters: json!({"type":"object","properties":{"city":{"type":"string"}},"required":["city"],"additionalProperties":false}),
            strict,
        },
    }
}

fn compat_request(choice: ToolChoice, strict: Option<bool>) -> ChatRequest {
    let mut request = ChatRequest::new(vec![Message::user("查两个城市🙂".into())]);
    request.tools = Some(vec![compat_tool(strict)]);
    request.tool_choice = Some(choice);
    request.response_format = Some(ResponseFormat::json_schema(
        "weather",
        json!({"type":"object","properties":{},"additionalProperties":false}),
    ));
    request
}

// 每个连接有界读取，只保存 JSON body，不保存 Authorization header。
async fn compat_server(
    responses: Vec<(u16, String, String)>,
) -> Result<(String, tokio::task::JoinHandle<Result<Vec<Value>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let handle = tokio::spawn(async move {
        let mut captured = Vec::new();
        for (status, kind, body) in responses {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                .await
                .map_err(|error| LlmError::NetworkError(error.to_string()))??;
            let mut data = Vec::new();
            loop {
                let count =
                    tokio::time::timeout(Duration::from_secs(3), socket.read_buf(&mut data))
                        .await
                        .map_err(|error| LlmError::NetworkError(error.to_string()))??;
                if count == 0 {
                    return Err(
                        LlmError::InvalidResponse("fixture truncated HTTP request".into()).into(),
                    );
                }
                let Ok(text) = std::str::from_utf8(&data) else {
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
                        .ok_or_else(|| {
                            LlmError::InvalidResponse("fixture requires content-length".into())
                        })?;
                    if payload.len() >= length {
                        captured.push(
                            serde_json::from_str(payload)
                                .map_err(|error| LlmError::InvalidResponse(error.to_string()))?,
                        );
                        break;
                    }
                }
            }
            socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await?;
            // 刻意在 Unicode 字节中间切片：操作的是 byte buffer，验证实际 SSE decoder 的 UTF-8 增量恢复。
            for chunk in body.as_bytes().chunks(7) {
                socket.write_all(chunk).await?;
            }
            socket.flush().await?;
        }
        Ok(captured)
    });
    Ok((url, handle))
}

fn compat_sse(events: &[Value], done: bool) -> String {
    let mut body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    if done {
        body.push_str("data: [DONE]\n\n");
    }
    body
}

async fn compat_captured(
    handle: tokio::task::JoinHandle<Result<Vec<Value>>>,
) -> Result<Vec<Value>> {
    handle
        .await
        .map_err(|error| LlmError::NetworkError(error.to_string()))?
}

#[tokio::test]
async fn chat_compat_http_tool_choice_strict_locations() -> Result<()> {
    let fixture = compat_fixture()?;
    for choice in [
        ToolChoice::Auto,
        ToolChoice::None,
        ToolChoice::Required,
        ToolChoice::function("lookup"),
    ] {
        for strict in [None, Some(false), Some(true)] {
            for stream in [false, true] {
                let body = if stream {
                    let events = fixture
                        .get("stream")
                        .and_then(Value::as_array)
                        .ok_or(LlmError::EmptyResponse)?;
                    compat_sse(events, true)
                } else {
                    fixture
                        .get("nonstream")
                        .ok_or(LlmError::EmptyResponse)?
                        .to_string()
                };
                let kind = if stream {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let (url, handle) = compat_server(vec![(200, kind.into(), body)]).await?;
                let client = compat_client(&url)?;
                let request = compat_request(choice.clone(), strict);
                if stream {
                    let chunks = client.chat_stream(request).await?.collect::<Vec<_>>().await;
                    let chunks = chunks.into_iter().collect::<Result<Vec<_>>>()?;
                    assert_eq!(
                        chunks
                            .last()
                            .and_then(|chunk| chunk.finish_reason.as_deref()),
                        Some("tool_calls")
                    );
                    assert_eq!(
                        chunks
                            .last()
                            .and_then(|chunk| chunk.usage.as_ref())
                            .and_then(|usage| usage.total_tokens),
                        Some(15)
                    );
                    let tool_deltas = chunks
                        .iter()
                        .flat_map(|chunk| chunk.delta.tool_calls.as_deref().unwrap_or_default())
                        .collect::<Vec<_>>();
                    assert!(
                        tool_deltas
                            .iter()
                            .any(|call| call.index == 1 && call.id.as_deref() == Some("call-1"))
                    );
                    assert!(
                        tool_deltas
                            .iter()
                            .any(|call| call.index == 0 && call.id.as_deref() == Some("call-0"))
                    );
                    for chunk in &chunks {
                        for block in chunk.delta.assistant_blocks.as_deref().unwrap_or_default() {
                            if block
                                .provider_source
                                .as_ref()
                                .and_then(|source| source.content_kind.as_deref())
                                == Some("content")
                            {
                                assert_eq!(block.phase, echo_core::agent::AssistantPhase::Unknown);
                            }
                        }
                    }
                } else {
                    let response = client.chat(request).await?;
                    assert_eq!(
                        response.message.reasoning_content.as_deref(),
                        Some("比较两个城市")
                    );
                    assert!(response.message.execution_status.is_none());
                    assert!(response.message.approved_assistant_blocks.is_none());
                    assert!(response.message.tool_result_is_error.is_none());
                }
                let captured = compat_captured(handle).await?;
                let request = captured.first().ok_or(LlmError::EmptyResponse)?;
                assert_eq!(request.get("tool_choice"), Some(&choice.to_openai_value()));
                assert_eq!(
                    request
                        .pointer("/tools/0/function/strict")
                        .and_then(Value::as_bool),
                    strict
                );
                assert_eq!(
                    request.pointer("/response_format/json_schema/strict"),
                    Some(&Value::Bool(true))
                );
                assert!(request.pointer("/tools/0/strict").is_none());
                eprintln!("CHAT_HTTP_CAPTURE tool_choice/strict {request}");
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn chat_compat_http_persistence_tool_roundtrip_and_multimodal() -> Result<()> {
    let fixture = compat_fixture()?;
    let response = fixture
        .get("nonstream")
        .ok_or(LlmError::EmptyResponse)?
        .to_string();
    let next = fixture
        .get("next")
        .ok_or(LlmError::EmptyResponse)?
        .to_string();
    let (url, handle) = compat_server(vec![
        (200, "application/json".into(), response),
        (200, "application/json".into(), next),
    ])
    .await?;
    let client = compat_client(&url)?;
    let first = client
        .chat(compat_request(ToolChoice::Required, Some(true)))
        .await?;
    let bytes = serde_json::to_vec(&first.message)
        .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    let mut restored: Message = serde_json::from_slice(&bytes)
        .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    restored.approved_assistant_blocks = restored.assistant_blocks.clone();
    let mut history = vec![Message::user("查天气".into()), restored];
    history.push(
        Message::tool_result("call-0".into(), "lookup".into(), "晴".into())
            .with_tool_result_is_error(false),
    );
    history.push(
        Message::tool_result("call-1".into(), "lookup".into(), "重试失败".into())
            .with_tool_result_is_error(true),
    );
    history.push(Message::user_multimodal(vec![ContentPart::ImageUrl {
        image_url: echo_core::llm::types::ImageUrl {
            url: "data:image/png;base64,aW1n".into(),
            detail: Some("low".into()),
        },
    }]));
    let result = client.chat(ChatRequest::new(history)).await?;
    assert_eq!(
        result.message.content.as_text().as_deref(),
        Some("杭州晴，上海多云")
    );
    let requests = compat_captured(handle).await?;
    let second = requests.get(1).ok_or(LlmError::EmptyResponse)?;
    assert_eq!(
        second
            .pointer("/messages/1/reasoning_content")
            .and_then(Value::as_str),
        Some("比较两个城市")
    );
    assert_eq!(
        second
            .pointer("/messages/1/tool_calls/0/id")
            .and_then(Value::as_str),
        Some("call-0")
    );
    assert_eq!(
        second
            .pointer("/messages/4/content/0/image_url/url")
            .and_then(Value::as_str),
        Some("data:image/png;base64,aW1n")
    );
    let encoded = second.to_string();
    for forbidden in [
        "assistant_blocks",
        "approved_assistant_blocks",
        "execution_status",
        "provider_source",
        "tool_result_is_error",
    ] {
        assert!(!encoded.contains(forbidden));
    }
    eprintln!("CHAT_HTTP_CAPTURE cold history {second}");
    Ok(())
}

#[tokio::test]
async fn chat_compat_http_rejects_bad_finish_eof_empty_and_arguments() -> Result<()> {
    let broken = [
        vec![
            json!({"id":"x","choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":null}]}),
        ],
        vec![
            json!({"id":"x","choices":[{"index":0,"delta":{"content":"cut"},"finish_reason":"length"}]}),
        ],
        vec![
            json!({"id":"x","choices":[{"index":0,"delta":{"reasoning_content":"private"},"finish_reason":"stop"}]}),
        ],
        vec![
            json!({"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"bad","function":{"name":"lookup","arguments":"not-json"}}]},"finish_reason":"tool_calls"}]}),
        ],
    ];
    for events in broken {
        for done in [false, true] {
            let (url, handle) = compat_server(vec![(
                200,
                "text/event-stream".into(),
                compat_sse(&events, done),
            )])
            .await?;
            let stream = compat_client(&url)?
                .chat_stream(ChatRequest::new(vec![Message::user("x".into())]))
                .await?;
            let chunks = stream.collect::<Vec<_>>().await;
            assert!(chunks.iter().any(Result::is_err));
            assert!(
                !chunks
                    .iter()
                    .filter_map(|chunk| chunk.as_ref().ok())
                    .any(|chunk| chunk.finish_reason.is_some())
            );
            let _captured = compat_captured(handle).await?;
        }
    }
    for response in [
        json!({"choices":[]}),
        json!({"choices":[{"message":{"role":"assistant","content":"cut"},"finish_reason":"length"}]}),
        json!({"choices":[{"message":{"role":"assistant","content":"","reasoning_content":"private"},"finish_reason":"stop"}]}),
    ] {
        let (url, handle) =
            compat_server(vec![(200, "application/json".into(), response.to_string())]).await?;
        assert!(
            compat_client(&url)?
                .chat(ChatRequest::default())
                .await
                .is_err()
        );
        let _captured = compat_captured(handle).await?;
    }
    let (url, handle) = compat_server(vec![(
        400,
        "application/json".into(),
        json!({"error":{"message":"unsupported combination"}}).to_string(),
    )])
    .await?;
    assert!(
        matches!(compat_client(&url)?.chat(ChatRequest::default()).await,Err(ReactError::Llm(error)) if matches!(error.as_ref(),LlmError::ApiError{status:400,..}))
    );
    let _captured = compat_captured(handle).await?;
    Ok(())
}

#[tokio::test]
async fn chat_compat_rejects_rich_tool_and_bad_parameters_before_http() -> Result<()> {
    // 无监听端口：所有错误必须来自请求适配，不能以网络失败伪装准确能力错误。
    let client = compat_client("http://127.0.0.1:9")?;
    for part in [
        ContentPart::ImageUrl {
            image_url: echo_core::llm::types::ImageUrl {
                url: "data:image/png;base64,aW1n".into(),
                detail: None,
            },
        },
        ContentPart::File {
            name: "a.txt".into(),
            content: "aGk=".into(),
        },
    ] {
        for stream in [false, true] {
            let mut tool = Message::tool_result("call-0".into(), "lookup".into(), String::new());
            tool.content = MessageContent::Parts(vec![part.clone()]);
            let request = ChatRequest::new(vec![tool]);
            let error = if stream {
                client.chat_stream(request).await.err()
            } else {
                client.chat(request).await.err()
            };
            assert!(
                error.is_some_and(|error| error.to_string().contains("supports text parts only"))
            );
        }
    }
    for choice in [
        ToolChoice::Required,
        ToolChoice::function("missing"),
        ToolChoice::function(""),
    ] {
        let request = ChatRequest {
            tool_choice: Some(choice),
            ..Default::default()
        };
        assert!(
            matches!(client.chat(request).await, Err(ReactError::Llm(error)) if matches!(error.as_ref(), LlmError::SerializationError(_)))
        );
    }
    let request = compat_request(ToolChoice::function("missing"), None);
    assert!(
        matches!(client.chat(request).await, Err(ReactError::Llm(error)) if matches!(error.as_ref(), LlmError::SerializationError(_)))
    );
    let fixture = compat_fixture()?;
    let (url, handle) = compat_server(vec![(
        200,
        "application/json".into(),
        fixture
            .get("next")
            .ok_or(LlmError::EmptyResponse)?
            .to_string(),
    )])
    .await?;
    let client = compat_client(&url)?;
    let mut request = ChatRequest::new(vec![Message::user("no tools".into())]);
    request.tool_choice = Some(ToolChoice::None);
    let _none_is_valid = client.chat(request.clone()).await?;
    let _captured = compat_captured(handle).await?;
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    request.cancel_token = Some(token);
    assert!(
        matches!(client.chat(request).await,Err(ReactError::Llm(error)) if matches!(error.as_ref(),LlmError::NetworkError(message) if message.contains("cancelled")))
    );
    Ok(())
}

#[tokio::test]
async fn chat_compat_http_stream_nonstream_same_next_request() -> Result<()> {
    use echo_core::llm::types::{FunctionCall, ToolCall};
    let fixture = compat_fixture()?;
    let sse = compat_sse(
        fixture
            .get("stream")
            .and_then(Value::as_array)
            .ok_or(LlmError::EmptyResponse)?,
        true,
    );
    let json = fixture
        .get("nonstream")
        .ok_or(LlmError::EmptyResponse)?
        .to_string();
    let (url, handle) = compat_server(vec![
        (200, "application/json".into(), json),
        (200, "text/event-stream".into(), sse),
        (
            200,
            "application/json".into(),
            fixture
                .get("next")
                .ok_or(LlmError::EmptyResponse)?
                .to_string(),
        ),
        (
            200,
            "application/json".into(),
            fixture
                .get("next")
                .ok_or(LlmError::EmptyResponse)?
                .to_string(),
        ),
    ])
    .await?;
    let client = compat_client(&url)?;
    let request = compat_request(ToolChoice::Required, None);
    let plain = client.chat(request.clone()).await?.message;
    let mut stream = client.chat_stream(request).await?;
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tools = std::collections::BTreeMap::new();
    let mut blocks = Vec::new();
    let mut terminal = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if let Some(text) = chunk.delta.content {
            content.push_str(&text);
        }
        if let Some(text) = chunk.delta.reasoning_content {
            reasoning.push_str(&text);
        }
        blocks.extend(chunk.delta.assistant_blocks.unwrap_or_default());
        for delta in chunk.delta.tool_calls.unwrap_or_default() {
            let call = tools.entry(delta.index).or_insert_with(|| ToolCall {
                id: String::new(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: String::new(),
                    arguments: String::new(),
                },
                provider_source: None,
            });
            if let Some(id) = delta.id {
                call.id = id;
            }
            if let Some(source) = delta.provider_source {
                call.provider_source = Some(source);
            }
            if let Some(function) = delta.function {
                if let Some(name) = function.name {
                    call.function.name.push_str(&name);
                }
                if let Some(args) = function.arguments {
                    call.function.arguments.push_str(&args);
                }
            }
        }
        if chunk.finish_reason.is_some() {
            terminal += 1;
        }
    }
    assert_eq!(terminal, 1);
    let mut streamed = Message::assistant(content);
    streamed.reasoning_content = Some(reasoning);
    streamed.tool_calls = Some(tools.into_values().collect());
    streamed.assistant_blocks = Some(blocks);
    let encoded = serde_json::to_vec(&streamed)
        .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    let restored = serde_json::from_slice(&encoded)
        .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    let _plain_next = client.chat(ChatRequest::new(vec![plain])).await?;
    let _stream_next = client.chat(ChatRequest::new(vec![restored])).await?;
    let captured = compat_captured(handle).await?;
    assert_eq!(captured.get(2), captured.get(3));
    assert!(captured.get(2).is_some());
    Ok(())
}

#[tokio::test]
async fn chat_compat_http_user_files_and_thinking_dialects() -> Result<()> {
    use echo_core::llm::{ThinkingConfig, ThinkingLevel};
    let fixture = compat_fixture()?;
    let response = fixture
        .get("next")
        .ok_or(LlmError::EmptyResponse)?
        .to_string();
    for (provider, model, thinking, expected) in [
        (
            "deepseek",
            "deepseek-v4-pro",
            ThinkingConfig::Level(ThinkingLevel::Xhigh),
            json!({"reasoning_effort":"high","thinking":{"type":"enabled"}}),
        ),
        (
            "bailian",
            "qwen3.8-omni-flash",
            ThinkingConfig::Level(ThinkingLevel::High),
            json!({"enable_thinking":true,"reasoning_effort":"high"}),
        ),
        (
            "bailian",
            "qwen3.8-omni-flash",
            ThinkingConfig::BudgetTokens(4096),
            json!({"enable_thinking":true,"thinking_budget":4096}),
        ),
        (
            "bailian",
            "qwen3-plus",
            ThinkingConfig::BudgetTokens(4096),
            json!({"enable_thinking":true,"thinking_budget":4096}),
        ),
        (
            "zhipu",
            "glm-5.2",
            ThinkingConfig::Disabled,
            json!({"thinking":{"type":"disabled"}}),
        ),
        (
            "ollama",
            "qwen3:32b",
            ThinkingConfig::Disabled,
            json!({"reasoning_effort":"none"}),
        ),
        (
            "gemini",
            "gemini-2.5-flash",
            ThinkingConfig::Level(ThinkingLevel::High),
            json!({"reasoning_effort":"high"}),
        ),
    ] {
        for stream in [false, true] {
            let events = vec![
                json!({"id":"dialect","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}),
            ];
            let (url, handle) = compat_server(vec![(
                200,
                if stream {
                    "text/event-stream"
                } else {
                    "application/json"
                }
                .into(),
                if stream {
                    compat_sse(&events, true)
                } else {
                    response.clone()
                },
            )])
            .await?;
            let client = OpenAiClient::new(LlmConfig::for_provider(
                provider,
                &url,
                "fixture-only",
                model,
                LlmApiProtocol::ChatCompletions,
            )?)?;
            let mut request = ChatRequest::new(vec![Message::user("x".into())]);
            request.thinking = Some(thinking.clone());
            if stream {
                let chunks = client.chat_stream(request).await?.collect::<Vec<_>>().await;
                let _valid = chunks.into_iter().collect::<Result<Vec<_>>>()?;
            } else {
                let _reply = client.chat(request).await?;
            }
            let requests = compat_captured(handle).await?;
            let wire = requests.first().ok_or(LlmError::EmptyResponse)?;
            for (key, value) in expected.as_object().ok_or(LlmError::EmptyResponse)? {
                assert_eq!(wire.get(key), Some(value));
            }
            assert!(wire.get("think").is_none());
            eprintln!("CHAT_HTTP_CAPTURE {provider}/{model} {wire}");
        }
    }
    for stream in [false, true] {
        let events = vec![
            json!({"id":"file","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}),
        ];
        let (url, handle) = compat_server(vec![(
            200,
            if stream {
                "text/event-stream"
            } else {
                "application/json"
            }
            .into(),
            if stream {
                compat_sse(&events, true)
            } else {
                response.clone()
            },
        )])
        .await?;
        let client = compat_client(&url)?;
        let request = ChatRequest::new(vec![Message::user_multimodal(vec![
            ContentPart::File {
                name: "paper.pdf".into(),
                content: "JVBERi0xLjc=".into(),
            },
            ContentPart::Text {
                text: "阅读🙂".into(),
            },
        ])]);
        if stream {
            let chunks = client.chat_stream(request).await?.collect::<Vec<_>>().await;
            let _valid = chunks.into_iter().collect::<Result<Vec<_>>>()?;
        } else {
            let _reply = client.chat(request).await?;
        }
        let requests = compat_captured(handle).await?;
        let wire = requests.first().ok_or(LlmError::EmptyResponse)?;
        assert_eq!(
            wire.pointer("/messages/0/content/0/file/file_data")
                .and_then(Value::as_str),
            Some("data:application/pdf;base64,JVBERi0xLjc=")
        );
        assert_eq!(
            wire.pointer("/messages/0/content/1/text")
                .and_then(Value::as_str),
            Some("阅读🙂")
        );
    }
    Ok(())
}

#[tokio::test]
async fn chat_compat_http_cancel_parked_response_in_both_modes() -> Result<()> {
    for streaming in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let token = tokio_util::sync::CancellationToken::new();
        let cancelling = token.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut data = Vec::new();
            let _read = socket.read_buf(&mut data).await?;
            // 对端已收到本次真实HTTP请求，才取消；不是预取消替代 in-flight 控制。
            cancelling.cancel();
            let mut tail = [0_u8; 1];
            let _closed =
                tokio::time::timeout(Duration::from_secs(2), socket.read(&mut tail)).await;
            std::io::Result::Ok(())
        });
        let client = compat_client(&url)?;
        let mut request = ChatRequest::new(vec![Message::user("cancel".into())]);
        request.cancel_token = Some(token);
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            if streaming {
                client.chat_stream(request).await.map(|_| ())
            } else {
                client.chat(request).await.map(|_| ())
            }
        })
        .await
        .map_err(|error| LlmError::NetworkError(error.to_string()))?;
        assert!(
            matches!(result,Err(ReactError::Llm(error)) if matches!(error.as_ref(),LlmError::NetworkError(message) if message.contains("cancelled")))
        );
        server
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))??;
    }
    Ok(())
}

#[tokio::test]
async fn chat_compat_http_deepseek_native_tool_images_and_terminal_usage() -> Result<()> {
    use echo_core::llm::types::{FunctionCall, ToolCall};
    use echo_core::llm::{ThinkingConfig, ThinkingLevel};
    for streaming in [false, true] {
        let usage = json!({"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"prompt_cache_hit_tokens":4,"prompt_cache_miss_tokens":6,"completion_tokens_details":{"reasoning_tokens":3}});
        let body = if streaming {
            compat_sse(
                &[
                    json!({"id":"deepseek-image","choices":[{"index":0,"delta":{"content":"看到了图片"},"finish_reason":null}]}),
                    json!({"id":"deepseek-image","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":usage}),
                ],
                true,
            )
        } else {
            json!({"id":"deepseek-image","choices":[{"index":0,"message":{"role":"assistant","content":"看到了图片"},"finish_reason":"stop"}],"usage":usage}).to_string()
        };
        let (url, handle) = compat_server(vec![(
            200,
            if streaming {
                "text/event-stream"
            } else {
                "application/json"
            }
            .into(),
            body,
        )])
        .await?;
        let client = OpenAiClient::new(
            LlmConfig::for_provider(
                "deepseek",
                &url,
                "fixture-only",
                "deepseek-v4-pro",
                LlmApiProtocol::ChatCompletions,
            )?
            .with_input_modalities(vec![ModelInputModality::Text, ModelInputModality::Image]),
        )?;
        let mut assistant = Message::assistant(String::new());
        assistant.tool_calls = Some(vec![ToolCall {
            id: "call-img".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "lookup".into(),
                arguments: "{}".into(),
            },
            provider_source: None,
        }]);
        let mut tool = Message::tool_result("call-img".into(), "lookup".into(), String::new());
        tool.content = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "截图🙂".into(),
            },
            ContentPart::ImageUrl {
                image_url: echo_core::llm::types::ImageUrl {
                    url: "data:image/png;base64,aW1n".into(),
                    detail: Some("original".into()),
                },
            },
            ContentPart::File {
                name: "shot.png".into(),
                content: "aW1n".into(),
            },
        ]);
        let request = ChatRequest::new(vec![Message::user("检查截图".into()), assistant, tool]);
        let usage = if streaming {
            let chunks = client.chat_stream(request).await?.collect::<Vec<_>>().await;
            let chunks = chunks.into_iter().collect::<Result<Vec<_>>>()?;
            assert_eq!(
                chunks
                    .iter()
                    .filter(|chunk| chunk.finish_reason.is_some())
                    .count(),
                1
            );
            chunks
                .last()
                .and_then(|chunk| chunk.usage.clone())
                .ok_or(LlmError::EmptyResponse)?
        } else {
            client
                .chat(request)
                .await?
                .usage
                .ok_or(LlmError::EmptyResponse)?
        };
        assert_eq!(
            usage
                .output_tokens_details
                .as_ref()
                .and_then(|details| details.reasoning_tokens),
            Some(3)
        );
        assert_eq!(usage.cached_prompt_tokens(), 4);
        let requests = compat_captured(handle).await?;
        let wire = requests.first().ok_or(LlmError::EmptyResponse)?;
        assert_eq!(
            wire.pointer("/messages/2/content/0/text")
                .and_then(Value::as_str),
            Some("截图🙂")
        );
        assert_eq!(
            wire.pointer("/messages/2/content/1/image_url/detail")
                .and_then(Value::as_str),
            Some("original")
        );
        assert_eq!(
            wire.pointer("/messages/2/content/2/file_data")
                .and_then(Value::as_str),
            Some("data:image/png;base64,aW1n")
        );
        assert!(wire.pointer("/messages/2/content/2/file").is_none());
        eprintln!("CHAT_HTTP_CAPTURE DeepSeek native tool images {wire}");
    }
    let client = OpenAiClient::new(LlmConfig::for_provider(
        "deepseek",
        "http://127.0.0.1:9",
        "fixture-only",
        "deepseek-v4-pro",
        LlmApiProtocol::ChatCompletions,
    )?)?;
    for thinking in [None, Some(ThinkingConfig::Level(ThinkingLevel::High))] {
        for choice in [ToolChoice::Required, ToolChoice::function("lookup")] {
            let mut request = compat_request(choice, None);
            request.response_format = None;
            request.thinking = thinking.clone();
            assert!(
                matches!(client.chat(request).await,Err(ReactError::Llm(error)) if matches!(error.as_ref(),LlmError::SerializationError(message) if message.contains("disabled thinking")))
            );
        }
    }
    let mut request = compat_request(ToolChoice::Auto, None);
    request.thinking = Some(ThinkingConfig::Disabled);
    assert!(
        matches!(client.chat(request).await,Err(ReactError::Llm(error)) if matches!(error.as_ref(),LlmError::SerializationError(message) if message.contains("not json_schema")))
    );
    let fixture = compat_fixture()?;
    for strict in [None, Some(true)] {
        let (url, handle) = compat_server(vec![(
            200,
            "application/json".into(),
            fixture
                .get("nonstream")
                .ok_or(LlmError::EmptyResponse)?
                .to_string(),
        )])
        .await?;
        let url = format!("{url}/beta");
        let client = OpenAiClient::new(LlmConfig::for_provider(
            "deepseek",
            &url,
            "fixture-only",
            "deepseek-v4-pro",
            LlmApiProtocol::ChatCompletions,
        )?)?;
        let mut request = compat_request(ToolChoice::function("lookup"), strict);
        request.thinking = Some(ThinkingConfig::Disabled);
        request.response_format = Some(ResponseFormat::JsonObject);
        let _reply = client.chat(request).await?;
        let captured = compat_captured(handle).await?;
        assert_eq!(
            captured
                .first()
                .and_then(|wire| wire.pointer("/thinking/type"))
                .and_then(Value::as_str),
            Some("disabled")
        );
    }
    Ok(())
}

#[tokio::test]
async fn chat_compat_http_exact_model_conditions_and_negative_overrides() -> Result<()> {
    use super::super::config::SourcedLlmConfig;
    use echo_core::llm::capabilities::{
        ModelFactConfidence, ModelFactMetadata, ModelFactSet, ModelFactSource, ModelProfileOverride,
    };
    use echo_core::llm::{ThinkingConfig, ThinkingProtocol};
    let fixture = compat_fixture()?;
    for streaming in [false, true] {
        let events = fixture
            .get("stream")
            .and_then(Value::as_array)
            .ok_or(LlmError::EmptyResponse)?;
        let response = if streaming {
            compat_sse(events, true)
        } else {
            fixture
                .get("nonstream")
                .ok_or(LlmError::EmptyResponse)?
                .to_string()
        };
        let (url, handle) = compat_server(vec![(
            200,
            if streaming {
                "text/event-stream"
            } else {
                "application/json"
            }
            .into(),
            response,
        )])
        .await?;
        let client = OpenAiClient::new(LlmConfig::for_provider(
            "deepseek",
            &url,
            "fixture-only",
            "deepseek-flash",
            LlmApiProtocol::ChatCompletions,
        )?)?;
        let mut request = compat_request(ToolChoice::function("lookup"), None);
        request.thinking = Some(ThinkingConfig::Disabled);
        request.response_format = None;
        if streaming {
            let chunks = client.chat_stream(request).await?.collect::<Vec<_>>().await;
            let _valid = chunks.into_iter().collect::<Result<Vec<_>>>()?;
        } else {
            let _reply = client.chat(request).await?;
        }
        let captured = compat_captured(handle).await?;
        assert_eq!(
            captured
                .first()
                .and_then(|wire| wire.pointer("/thinking/type"))
                .and_then(Value::as_str),
            Some("disabled")
        );
    }
    for model in ["qwen3.5-122b-a10b", "qwen3.5-27b"] {
        let client = OpenAiClient::new(LlmConfig::for_provider(
            "bailian",
            "http://127.0.0.1:9",
            "fixture-only",
            model,
            LlmApiProtocol::ChatCompletions,
        )?)?;
        assert!(
            matches!(client.chat(ChatRequest::default()).await,Err(ReactError::Llm(error)) if matches!(error.as_ref(),LlmError::SerializationError(message) if message.contains("streaming only")))
        );
        let response = compat_sse(
            &[
                json!({"id":"qwen","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}),
            ],
            true,
        );
        let (url, handle) =
            compat_server(vec![(200, "text/event-stream".into(), response)]).await?;
        let client = OpenAiClient::new(LlmConfig::for_provider(
            "bailian",
            &url,
            "fixture-only",
            model,
            LlmApiProtocol::ChatCompletions,
        )?)?;
        let chunks = client
            .chat_stream(ChatRequest::new(vec![Message::user("x".into())]))
            .await?
            .collect::<Vec<_>>()
            .await;
        let _valid = chunks.into_iter().collect::<Result<Vec<_>>>()?;
        let _captured = compat_captured(handle).await?;
    }
    for (provider, model) in [
        ("gemini", "gemini-2.5-flash"),
        ("custom", "gemini-2.5-flash"),
        ("deepseek", "deepseek-flash"),
    ] {
        for protocol in [ThinkingProtocol::None, ThinkingProtocol::ModelManaged] {
            let fact = ModelFactSet::new(
                ModelFactMetadata::new(
                    ModelFactSource::CallerOverride,
                    "negative exact fixture",
                    "v1",
                    std::time::SystemTime::now(),
                    None,
                    ModelFactConfidence::VERIFIED,
                ),
                None,
                ModelProfileOverride {
                    thinking_protocol: Some(protocol),
                    ..Default::default()
                },
            );
            let client = SourcedLlmConfig::new(LlmConfig::for_provider(
                provider,
                "http://127.0.0.1:9",
                "fixture-only",
                model,
                LlmApiProtocol::ChatCompletions,
            )?)
            .with_model_override(fact)
            .build_client()?;
            let mut request = ChatRequest::new(vec![Message::user("x".into())]);
            request.thinking = Some(ThinkingConfig::medium());
            assert!(
                matches!(client.chat(request).await,Err(ReactError::Llm(error)) if matches!(error.as_ref(),LlmError::SerializationError(message) if message.contains("no compatible request-side control")))
            );
        }
    }
    for provider in ["custom", "ollama"] {
        let (url, handle) = compat_server(vec![(
            200,
            "application/json".into(),
            fixture
                .get("next")
                .ok_or(LlmError::EmptyResponse)?
                .to_string(),
        )])
        .await?;
        let client = OpenAiClient::new(LlmConfig::for_provider(
            provider,
            &url,
            "fixture-only",
            "gpt-5.6-local",
            LlmApiProtocol::ChatCompletions,
        )?)?;
        let mut request = ChatRequest::new(vec![Message::user("temperature".into())]);
        request.temperature = Some(0.25);
        request.max_tokens = Some(33);
        let _reply = client.chat(request).await?;
        let captured = compat_captured(handle).await?;
        let wire = captured.first().ok_or(LlmError::EmptyResponse)?;
        assert_eq!(wire.get("temperature").and_then(Value::as_f64), Some(0.25));
        assert_eq!(wire.get("max_tokens").and_then(Value::as_u64), Some(33));
        assert!(wire.get("max_completion_tokens").is_none());
    }
    Ok(())
}
