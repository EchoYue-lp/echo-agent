// D2正式variant与分类器存在后才可编译；不新增variant、不打印partial正文。
fn assert_typed_chat_incomplete(error: &ReactError, reason: Option<&str>) -> Result<()> {
    let ReactError::Llm(inner) = error else {
        return Err(LlmError::InvalidResponse("expected LlmError".into()).into());
    };
    let LlmError::IncompleteResponse { partial } = inner.as_ref() else {
        return Err(LlmError::InvalidResponse("truncation lost typed partial".into()).into());
    };
    assert!(partial.execution_status.is_none());
    assert!(partial.approved_assistant_blocks.is_none());
    let metadata = chat_metadata_value(partial)?;
    assert_eq!(metadata.get("stop_reason").and_then(Value::as_str), reason);
    assert_eq!(partial.content.as_text().as_deref(), Some("正文🙂"));
    assert_eq!(partial.reasoning_content.as_deref(), Some("原生推理"));
    assert!(
        !error.to_string().contains("正文🙂"),
        "Display leaked unchecked partial body"
    );
    let failure = echo_core::error::AgentFailure::try_from(error)?;
    assert!(!failure.retryable);
    assert_eq!(failure.http_status, None);
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_non_success_finish_retains_partial_without_success() -> Result<()> {
    let fixture = chat_metadata_fixture()?;
    for case in fixture
        .get("incomplete")
        .and_then(Value::as_array)
        .ok_or(LlmError::EmptyResponse)?
    {
        let reason = case
            .get("id")
            .and_then(Value::as_str)
            .ok_or(LlmError::EmptyResponse)?;
        for streaming in [false, true] {
            let body = if streaming {
                compat_sse(
                    case.get("stream")
                        .and_then(Value::as_array)
                        .ok_or(LlmError::EmptyResponse)?,
                    true,
                )
            } else {
                case.get("nonstream")
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
                body,
            )])
            .await?;
            let client = compat_client(&url)?;
            if streaming {
                let items = client
                    .chat_stream(ChatRequest::new(vec![Message::user("partial".into())]))
                    .await?
                    .collect::<Vec<_>>()
                    .await;
                let error = items
                    .iter()
                    .find_map(|item| item.as_ref().err())
                    .ok_or(LlmError::EmptyResponse)?;
                assert_typed_chat_incomplete(error, Some(reason))?;
                for chunk in items.iter().filter_map(|item| item.as_ref().ok()) {
                    assert!(chunk.finish_reason.is_none() && chunk.usage.is_none());
                }
            } else {
                let error = client
                    .chat(ChatRequest::new(vec![Message::user("partial".into())]))
                    .await
                    .err()
                    .ok_or(LlmError::EmptyResponse)?;
                assert_typed_chat_incomplete(&error, Some(reason))?;
            }
            let _captured = compat_captured(handle).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_eof_keeps_snapshot_but_late_provider_error_keeps_typed8() -> Result<()> {
    let fixture = chat_metadata_fixture()?;
    let events = vec![
        json!({"id":"native-meta-1","model":"wire-model","choices":[{"index":0,"delta":{"content":"正文🙂","reasoning_content":"原生推理"},"finish_reason":null}]}),
        json!({"id":"native-meta-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3,"native_extra":{"null":null}}}),
    ];
    for late_error in [false, true] {
        let mut input = events.clone();
        if late_error {
            input.push(
                fixture
                    .get("late_error")
                    .ok_or(LlmError::EmptyResponse)?
                    .clone(),
            );
        }
        let (url, handle) = compat_server(vec![(
            200,
            "text/event-stream".into(),
            compat_sse(&input, late_error),
        )])
        .await?;
        let items = compat_client(&url)?
            .chat_stream(ChatRequest::new(vec![Message::user("EOF".into())]))
            .await?
            .collect::<Vec<_>>()
            .await;
        let error = items
            .iter()
            .find_map(|item| item.as_ref().err())
            .ok_or(LlmError::EmptyResponse)?;
        if late_error {
            assert!(
                matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::ProviderError{retryable:false,code:Some(code),..} if code==&json!(37)))
            );
        } else {
            assert_typed_chat_incomplete(error, Some("stop"))?;
        }
        for chunk in items.iter().filter_map(|item| item.as_ref().ok()) {
            assert!(chunk.finish_reason.is_none() && chunk.usage.is_none());
        }
        let _captured = compat_captured(handle).await?;
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_protocol_damage_http_and_empty_keep_original_errors() -> Result<()> {
    let partial =
        json!({"id":"bad-json","choices":[{"index":0,"delta":{"content":"before malformed"}}]});
    let malformed = format!("data: {partial}\n\ndata: {{broken-json}}\n\n");
    let (url, handle) = compat_server(vec![(200, "text/event-stream".into(), malformed)]).await?;
    let items = compat_client(&url)?
        .chat_stream(ChatRequest::new(vec![Message::user("malformed".into())]))
        .await?
        .collect::<Vec<_>>()
        .await;
    let error = items
        .iter()
        .find_map(|item| item.as_ref().err())
        .ok_or(LlmError::EmptyResponse)?;
    assert!(
        matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::InvalidResponse(_)))
    );
    let _captured = compat_captured(handle).await?;
    for body in ["data: [DONE]\n\n", ""] {
        let (url, handle) =
            compat_server(vec![(200, "text/event-stream".into(), body.into())]).await?;
        let client = compat_client(&url)?;
        let opened = client
            .chat_stream(ChatRequest::new(vec![Message::user("empty".into())]))
            .await;
        let error = match opened {
            Err(error) => error,
            Ok(stream) => stream
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .find_map(Result::err)
                .ok_or(LlmError::EmptyResponse)?,
        };
        assert!(
            matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::InvalidResponse(_)))
        );
        let _captured = compat_captured(handle).await?;
    }
    let (url, handle) = compat_server(vec![(
        429,
        "application/json".into(),
        json!({"error":{"message":"real HTTP rate limit"}}).to_string(),
    )])
    .await?;
    let error = compat_client(&url)?
        .chat(ChatRequest::default())
        .await
        .err()
        .ok_or(LlmError::EmptyResponse)?;
    assert!(
        matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::ApiError{status:429,..}))
    );
    let _captured = compat_captured(handle).await?;
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_unfinished_tool_fragment_is_observation_only() -> Result<()> {
    let events = vec![
        json!({"id":"partial-tool","model":"native-model","choices":[{"index":0,"delta":{"reasoning_content":"核对参数","content":"部分正文","tool_calls":[{"index":2,"id":"call-partial","type":"function","function":{"name":"lookup","arguments":"{\"city\":"}}]},"finish_reason":null}]}),
        json!({"id":"partial-tool","choices":[{"index":0,"delta":{},"finish_reason":"length"}],"usage":{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7}}),
    ];
    let (url, handle) = compat_server(vec![(
        200,
        "text/event-stream".into(),
        compat_sse(&events, true),
    )])
    .await?;
    let items = compat_client(&url)?
        .chat_stream(ChatRequest::new(vec![Message::user("partial tool".into())]))
        .await?
        .collect::<Vec<_>>()
        .await;
    let error = items
        .iter()
        .find_map(|item| item.as_ref().err())
        .ok_or(LlmError::EmptyResponse)?;
    let ReactError::Llm(inner) = error else {
        return Err(LlmError::EmptyResponse.into());
    };
    let LlmError::IncompleteResponse { partial } = inner.as_ref() else {
        return Err(LlmError::EmptyResponse.into());
    };
    let call = partial
        .tool_calls
        .as_deref()
        .and_then(|calls| calls.first())
        .ok_or(LlmError::EmptyResponse)?;
    assert_eq!(call.id, "call-partial");
    assert_eq!(call.function.arguments, "{\"city\":");
    assert_eq!(
        call.provider_source
            .as_ref()
            .map(|source| source.output_index),
        Some(2)
    );
    assert_eq!(partial.reasoning_content.as_deref(), Some("核对参数"));
    assert!(partial.execution_status.is_none() && partial.approved_assistant_blocks.is_none());
    assert!(
        items
            .iter()
            .filter_map(|item| item.as_ref().ok())
            .all(|chunk| chunk.finish_reason.is_none() && chunk.usage.is_none())
    );
    let failure = echo_core::error::AgentFailure::try_from(error)?;
    assert!(!failure.retryable);
    assert_eq!(failure.code, "llm_incomplete_response");
    let _captured = compat_captured(handle).await?;
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_nonstream_reported_error_precedes_incomplete_diagnostic() -> Result<()> {
    let wire = json!({"id":"failed-native","model":"wire-model","choices":[{"index":0,"message":{"role":"assistant","content":"unchecked partial"},"finish_reason":"length"}],"usage":{"total_tokens":7},"error":{"type":"overloaded_error","code":37,"message":"actual upstream failure"}});
    let (url, handle) =
        compat_server(vec![(200, "application/json".into(), wire.to_string())]).await?;
    let error = compat_client(&url)?
        .chat(ChatRequest::new(vec![Message::user("first cause".into())]))
        .await
        .err()
        .ok_or(LlmError::EmptyResponse)?;
    assert!(
        matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::ProviderError{retryable:false,code:Some(code),message,..} if code==&json!(37) && message=="actual upstream failure"))
    );
    let _captured = compat_captured(handle).await?;
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_stop_missing_and_explicit_null_stay_distinct() -> Result<()> {
    for stop in [None, Some(Value::Null), Some(json!("new_vendor_reason"))] {
        for streaming in [false, true] {
            let mut choice = json!({"index":0});
            let key = if streaming { "delta" } else { "message" };
            let mut content = json!({"content":"正文🙂","reasoning_content":"原生推理"});
            if !streaming {
                content
                    .as_object_mut()
                    .ok_or(LlmError::EmptyResponse)?
                    .insert("role".into(), json!("assistant"));
            }
            choice
                .as_object_mut()
                .ok_or(LlmError::EmptyResponse)?
                .insert(key.into(), content);
            if let Some(stop) = stop.as_ref() {
                choice
                    .as_object_mut()
                    .ok_or(LlmError::EmptyResponse)?
                    .insert("finish_reason".into(), stop.clone());
            }
            let wire = json!({"id":"nullable-stop","model":null,"usage":null,"choices":[choice]});
            let body = if streaming {
                compat_sse(&[wire], true)
            } else {
                wire.to_string()
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
            let client = compat_client(&url)?;
            let error = if streaming {
                client
                    .chat_stream(ChatRequest::new(vec![Message::user(
                        "nullable stop".into(),
                    )]))
                    .await?
                    .collect::<Vec<_>>()
                    .await
                    .into_iter()
                    .find_map(Result::err)
                    .ok_or(LlmError::EmptyResponse)?
            } else {
                client
                    .chat(ChatRequest::new(vec![Message::user(
                        "nullable stop".into(),
                    )]))
                    .await
                    .err()
                    .ok_or(LlmError::EmptyResponse)?
            };
            let ReactError::Llm(inner) = &error else {
                return Err(LlmError::EmptyResponse.into());
            };
            let LlmError::IncompleteResponse { partial } = inner.as_ref() else {
                return Err(LlmError::EmptyResponse.into());
            };
            let observed = chat_metadata_value(partial)?;
            assert_eq!(observed.get("stop_reason"), stop.as_ref());
            assert_eq!(observed.get("model"), Some(&Value::Null));
            assert_eq!(observed.get("usage"), Some(&Value::Null));
            assert_typed_chat_incomplete(&error, stop.as_ref().and_then(Value::as_str))?;
            let _captured = compat_captured(handle).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_missing_or_late_native_id_preserves_tool_index() -> Result<()> {
    for late_id in [false, true] {
        let mut events = vec![
            json!({"choices":[{"index":0,"delta":{"content":"partial tool","tool_calls":[{"index":9,"id":"call-index-nine","type":"function","function":{"name":"lookup","arguments":"{\"city\":"}},{"index":5,"id":"call-index-five","type":"function","function":{"name":"lookup","arguments":"{\"city\":"}}]},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
        ];
        if late_id {
            events.push(json!({"id":"native-id-late","choices":[],"usage":{"total_tokens":3}}));
        }
        let (url, handle) = compat_server(vec![(
            200,
            "text/event-stream".into(),
            compat_sse(&events, false),
        )])
        .await?;
        let items = compat_client(&url)?
            .chat_stream(ChatRequest::new(vec![Message::user(
                "native id boundary".into(),
            )]))
            .await?
            .collect::<Vec<_>>()
            .await;
        let error = items
            .iter()
            .find_map(|item| item.as_ref().err())
            .ok_or(LlmError::EmptyResponse)?;
        let ReactError::Llm(inner) = error else {
            return Err(LlmError::EmptyResponse.into());
        };
        let LlmError::IncompleteResponse { partial } = inner.as_ref() else {
            return Err(LlmError::EmptyResponse.into());
        };
        let bytes = serde_json::to_vec(partial)
            .map_err(|error| LlmError::SerializationError(error.to_string()))?;
        let restored: Message = serde_json::from_slice(&bytes)
            .map_err(|error| LlmError::SerializationError(error.to_string()))?;
        let calls = restored
            .tool_calls
            .as_deref()
            .ok_or(LlmError::EmptyResponse)?;
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls
                .iter()
                .filter_map(|call| call
                    .provider_source
                    .as_ref()
                    .map(|source| source.output_index))
                .collect::<Vec<_>>(),
            vec![5, 9]
        );
        assert_eq!(
            calls
                .iter()
                .map(|call| call.id.as_str())
                .collect::<Vec<_>>(),
            vec!["call-index-five", "call-index-nine"]
        );
        for call in calls {
            let source = call
                .provider_source
                .as_ref()
                .ok_or(LlmError::EmptyResponse)?;
            assert_eq!(
                source.request_id,
                if late_id { "native-id-late" } else { "" }
            );
            assert_eq!(call.function.arguments, "{\"city\":");
        }
        let metadata = chat_metadata_value(&restored)?;
        if late_id {
            assert_eq!(
                metadata.get("request_id").and_then(Value::as_str),
                Some("native-id-late")
            );
        } else {
            assert!(
                restored
                    .provider_response
                    .as_ref()
                    .and_then(|value| value.request_id.as_ref())
                    .is_none()
            );
            assert!(metadata.get("request_id").is_none_or(Value::is_null));
        }
        assert!(
            restored.execution_status.is_none() && restored.approved_assistant_blocks.is_none()
        );
        let failure = echo_core::error::AgentFailure::try_from(error)?;
        assert_eq!(failure.code, "llm_incomplete_response");
        assert!(!failure.retryable && failure.http_status.is_none());
        let _captured = compat_captured(handle).await?;
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_empty_source_partial_replay_rejected_before_http() -> Result<()> {
    let events = vec![
        json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":9,"id":"call-nine","type":"function","function":{"name":"lookup","arguments":"{\"city\":"}},{"index":5,"id":"call-five","type":"function","function":{"name":"lookup","arguments":"{\"city\":"}}]},"finish_reason":null}]}),
    ];
    let (url, handle) = compat_server(vec![(
        200,
        "text/event-stream".into(),
        compat_sse(&events, false),
    )])
    .await?;
    let items = compat_client(&url)?
        .chat_stream(ChatRequest::new(vec![Message::user(
            "observe partial".into(),
        )]))
        .await?
        .collect::<Vec<_>>()
        .await;
    let error = items
        .into_iter()
        .find_map(Result::err)
        .ok_or(LlmError::EmptyResponse)?;
    let ReactError::Llm(inner) = error else {
        return Err(LlmError::EmptyResponse.into());
    };
    let LlmError::IncompleteResponse { partial } = *inner else {
        return Err(LlmError::EmptyResponse.into());
    };
    let bytes = serde_json::to_vec(&partial)
        .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    let restored: Message = serde_json::from_slice(&bytes)
        .map_err(|error| LlmError::SerializationError(error.to_string()))?;
    assert_eq!(
        restored
            .tool_calls
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(|call| call
                .provider_source
                .as_ref()
                .map(|source| source.output_index))
            .collect::<Vec<_>>(),
        vec![5, 9]
    );
    let _first_capture = compat_captured(handle).await?;
    for streaming in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let client = compat_client(&endpoint)?;
        let request = ChatRequest::new(vec![
            Message::user("must not replay".into()),
            restored.clone(),
        ]);
        let rejected = tokio::time::timeout(Duration::from_secs(1), async {
            if streaming {
                client.chat_stream(request).await.err()
            } else {
                client.chat(request).await.err()
            }
        })
        .await
        .map_err(|error| LlmError::NetworkError(error.to_string()))?
        .ok_or(LlmError::EmptyResponse)?;
        assert!(
            matches!(rejected,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::SerializationError(message) if message.contains("D2 partial tool source cannot be replayed")))
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err(),
            "rejected partial opened an HTTP connection"
        );
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_complete_external_source_none_remains_valid() -> Result<()> {
    let fixture = chat_metadata_fixture()?;
    let response = fixture
        .pointer("/nonstream/0/wire")
        .ok_or(LlmError::EmptyResponse)?
        .clone();
    for streaming in [false, true] {
        let events = vec![
            json!({"id":"complete","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}),
        ];
        let (url, handle) = compat_server(vec![(
            200,
            if streaming {
                "text/event-stream"
            } else {
                "application/json"
            }
            .into(),
            if streaming {
                compat_sse(&events, true)
            } else {
                response.to_string()
            },
        )])
        .await?;
        let complete:Message=serde_json::from_value(json!({"role":"assistant","content":null,"tool_calls":[{"id":"external-complete","type":"function","function":{"name":"lookup","arguments":"{}"}}]})).map_err(|error|LlmError::SerializationError(error.to_string()))?;
        assert!(
            complete
                .tool_calls
                .as_deref()
                .and_then(|calls| calls.first())
                .is_some_and(|call| call.provider_source.is_none())
        );
        let request = ChatRequest::new(vec![
            Message::user("external".into()),
            complete,
            Message::tool_result("external-complete".into(), "lookup".into(), "ok".into()),
        ]);
        let client = compat_client(&url)?;
        if streaming {
            let items = client.chat_stream(request).await?.collect::<Vec<_>>().await;
            let _valid = items.into_iter().collect::<Result<Vec<_>>>()?;
        } else {
            let _valid = client.chat(request).await?;
        }
        let requests = compat_captured(handle).await?;
        assert_eq!(
            requests
                .first()
                .and_then(|wire| wire.pointer("/messages/1/tool_calls/0/id"))
                .and_then(Value::as_str),
            Some("external-complete")
        );
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_unknown_then_known_then_changed_id_stays_protocol_error() -> Result<()> {
    let events = vec![
        json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":5,"id":"call-five","function":{"name":"lookup","arguments":"{}"}}]}}]}),
        json!({"id":"native-id-first","choices":[],"usage":{"total_tokens":1}}),
        json!({"id":"native-id-second","choices":[],"usage":{"total_tokens":2}}),
    ];
    let (url, handle) = compat_server(vec![(
        200,
        "text/event-stream".into(),
        compat_sse(&events, true),
    )])
    .await?;
    let items = compat_client(&url)?
        .chat_stream(ChatRequest::new(vec![Message::user(
            "request identity".into(),
        )]))
        .await?
        .collect::<Vec<_>>()
        .await;
    let error = items
        .iter()
        .find_map(|item| item.as_ref().err())
        .ok_or(LlmError::EmptyResponse)?;
    assert!(
        matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::InvalidResponse(message) if message.contains("response id changed")))
    );
    assert!(
        items
            .iter()
            .filter_map(|item| item.as_ref().ok())
            .all(|chunk| chunk.finish_reason.is_none() && chunk.usage.is_none())
    );
    let _captured = compat_captured(handle).await?;
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_required_native_id_missing_never_becomes_success() -> Result<()> {
    let fixture = chat_metadata_fixture()?;
    for streaming in [false, true] {
        let body = if streaming {
            compat_sse(
                &[
                    json!({"choices":[{"index":0,"delta":{"content":"正文🙂","reasoning_content":"原生推理"},"finish_reason":null}]}),
                    json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
                ],
                true,
            )
        } else {
            let mut wire = fixture
                .pointer("/nonstream/0/wire")
                .ok_or(LlmError::EmptyResponse)?
                .clone();
            wire.as_object_mut()
                .ok_or(LlmError::EmptyResponse)?
                .remove("id");
            wire.to_string()
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
        let client = compat_client(&url)?;
        let error = if streaming {
            let items = client
                .chat_stream(ChatRequest::new(vec![Message::user(
                    "required native id".into(),
                )]))
                .await?
                .collect::<Vec<_>>()
                .await;
            assert!(
                items
                    .iter()
                    .filter_map(|item| item.as_ref().ok())
                    .all(|chunk| chunk.finish_reason.is_none() && chunk.usage.is_none())
            );
            items
                .into_iter()
                .find_map(Result::err)
                .ok_or(LlmError::EmptyResponse)?
        } else {
            client
                .chat(ChatRequest::new(vec![Message::user(
                    "required native id".into(),
                )]))
                .await
                .err()
                .ok_or(LlmError::EmptyResponse)?
        };
        let ReactError::Llm(inner) = &error else {
            return Err(LlmError::EmptyResponse.into());
        };
        let LlmError::IncompleteResponse { partial } = inner.as_ref() else {
            return Err(LlmError::EmptyResponse.into());
        };
        let metadata = chat_metadata_value(partial)?;
        assert!(metadata.get("request_id").is_none_or(Value::is_null));
        assert!(!metadata.to_string().contains("chat:"));
        assert_typed_chat_incomplete(&error, Some("stop"))?;
        let _captured = compat_captured(handle).await?;
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_empty_native_wire_id_is_protocol_error() -> Result<()> {
    for streaming in [false, true] {
        let wire = json!({"id":"","choices":[{"index":0,"message":{"role":"assistant","content":"body"},"delta":{"content":"body"},"finish_reason":"stop"}]});
        let (url, handle) = compat_server(vec![(
            200,
            if streaming {
                "text/event-stream"
            } else {
                "application/json"
            }
            .into(),
            if streaming {
                compat_sse(&[wire], true)
            } else {
                wire.to_string()
            },
        )])
        .await?;
        let client = compat_client(&url)?;
        let error = if streaming {
            client
                .chat_stream(ChatRequest::new(vec![Message::user(
                    "empty native id".into(),
                )]))
                .await?
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .find_map(Result::err)
                .ok_or(LlmError::EmptyResponse)?
        } else {
            client
                .chat(ChatRequest::new(vec![Message::user(
                    "empty native id".into(),
                )]))
                .await
                .err()
                .ok_or(LlmError::EmptyResponse)?
        };
        assert!(
            matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::InvalidResponse(message) if message.contains("native response id was empty")))
        );
        let _captured = compat_captured(handle).await?;
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_late_real_id_updates_success_coordinates_without_repeating_arguments()
-> Result<()> {
    let events = vec![
        json!({"choices":[{"index":0,"delta":{"content":"checking","tool_calls":[{"index":9,"id":"call-nine","type":"function","function":{"name":"lookup","arguments":"{}"}},{"index":5,"id":"call-five","type":"function","function":{"name":"lookup","arguments":"{}"}}]},"finish_reason":null}]}),
        json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
        json!({"id":"real-id-late","choices":[],"usage":{"total_tokens":3}}),
    ];
    let (url, handle) = compat_server(vec![(
        200,
        "text/event-stream".into(),
        compat_sse(&events, true),
    )])
    .await?;
    let items = compat_client(&url)?
        .chat_stream(ChatRequest::new(vec![Message::user(
            "late success coordinates".into(),
        )]))
        .await?
        .collect::<Vec<_>>()
        .await;
    let chunks = items.into_iter().collect::<Result<Vec<_>>>()?;
    let final_chunk = chunks.last().ok_or(LlmError::EmptyResponse)?;
    assert_eq!(final_chunk.finish_reason.as_deref(), Some("tool_calls"));
    assert!(
        final_chunk
            .delta
            .assistant_blocks
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|block| {
                block.complete
                    && block.provider_source.as_ref().is_some_and(|source| {
                        source.content_kind.as_deref() == Some("content")
                            && source.request_id == "real-id-late"
                    })
            }),
        "late native id was not attached to the closed original content block"
    );
    let updates = final_chunk
        .delta
        .tool_calls
        .as_deref()
        .ok_or(LlmError::EmptyResponse)?;
    assert_eq!(
        updates.iter().map(|call| call.index).collect::<Vec<_>>(),
        vec![5, 9]
    );
    for update in updates {
        assert!(update.id.is_none() && update.function.is_none() && update.call_type.is_none());
        assert_eq!(
            update
                .provider_source
                .as_ref()
                .map(|source| source.request_id.as_str()),
            Some("real-id-late")
        );
    }
    let args = chunks
        .iter()
        .flat_map(|chunk| chunk.delta.tool_calls.as_deref().unwrap_or_default())
        .filter_map(|call| {
            call.function
                .as_ref()
                .and_then(|function| function.arguments.as_deref())
        })
        .collect::<Vec<_>>();
    assert_eq!(args, vec!["{}", "{}"]);
    for chunk in &chunks {
        if chunk
            .delta
            .provider_response
            .as_ref()
            .is_some_and(|record| record.request_id.is_none())
        {
            assert!(
                chunk
                    .delta
                    .tool_calls
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .all(|call| call.provider_source.is_none())
            );
            assert!(
                chunk
                    .delta
                    .assistant_blocks
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .all(|block| !block.request_id.is_empty() && block.provider_source.is_none())
            );
        }
    }
    let _captured = compat_captured(handle).await?;
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_metadata_only_eof_or_done_preserves_observation() -> Result<()> {
    let samples = vec![
        json!({"id":"metadata-only-native","model":"wire-model","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":7,"total_tokens":10,"native_extra":{"max_u64":u64::MAX,"nullable":null}}}),
        json!({"model":null,"choices":[]}),
        json!({"choices":[],"usage":null}),
    ];
    for native in samples {
        for done in [false, true] {
            let (url, handle) = compat_server(vec![(
                200,
                "text/event-stream".into(),
                compat_sse(std::slice::from_ref(&native), done),
            )])
            .await?;
            let items = compat_client(&url)?
                .chat_stream(ChatRequest::new(vec![Message::user(
                    "metadata-only termination".into(),
                )]))
                .await?
                .collect::<Vec<_>>()
                .await;
            assert!(
                items
                    .iter()
                    .filter_map(|item| item.as_ref().ok())
                    .any(|chunk| chunk.delta.provider_response.is_some())
            );
            for chunk in items.iter().filter_map(|item| item.as_ref().ok()) {
                assert!(chunk.finish_reason.is_none() && chunk.usage.is_none());
                assert!(
                    chunk.delta.content.is_none()
                        && chunk.delta.reasoning_content.is_none()
                        && chunk.delta.tool_calls.is_none()
                );
            }
            let error = items
                .iter()
                .find_map(|item| item.as_ref().err())
                .ok_or(LlmError::EmptyResponse)?;
            let ReactError::Llm(inner) = error else {
                return Err(LlmError::EmptyResponse.into());
            };
            let LlmError::IncompleteResponse { partial } = inner.as_ref() else {
                return Err(LlmError::InvalidResponse(
                    "accepted metadata-only observation lost typed partial".into(),
                )
                .into());
            };
            let observed = chat_metadata_value(partial)?;
            assert_eq!(
                observed.get("protocol").and_then(Value::as_str),
                Some("chat_completions")
            );
            assert_eq!(
                observed.get("provider").and_then(Value::as_str),
                Some("openai")
            );
            assert_eq!(observed.get("request_id"), native.get("id"));
            assert_eq!(observed.get("model"), native.get("model"));
            assert_eq!(observed.get("usage"), native.get("usage"));
            assert!(observed.get("stop_reason").is_none());
            assert!(
                partial
                    .content
                    .as_text()
                    .as_deref()
                    .is_none_or(str::is_empty)
            );
            assert!(
                partial.tool_calls.is_none()
                    && partial.reasoning_content.is_none()
                    && partial.reasoning_blocks.is_none()
            );
            assert!(
                partial.assistant_blocks.is_none()
                    && partial.execution_status.is_none()
                    && partial.approved_assistant_blocks.is_none()
            );
            assert_eq!(inner.to_string(), "Incomplete response from LLM");
            let failure = echo_core::error::AgentFailure::try_from(error)?;
            assert_eq!(failure.code, "llm_incomplete_response");
            assert!(!failure.retryable && failure.http_status.is_none());
            let _captured = compat_captured(handle).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_empty_or_forged_only_input_remains_original_invalid() -> Result<()> {
    let bodies = vec![
        String::new(),
        "data: [DONE]\n\n".into(),
        compat_sse(&[json!({})], true),
        compat_sse(&[json!({"choices":[]})], true),
        compat_sse(
            &[
                json!({"provider_response":{"protocol":"chat_completions","provider":"forged","request_id":"fake"},"execution_status":"fake-completion","choices":[]}),
            ],
            true,
        ),
    ];
    for body in bodies {
        let (url, handle) = compat_server(vec![(200, "text/event-stream".into(), body)]).await?;
        let client = compat_client(&url)?;
        let error = match client
            .chat_stream(ChatRequest::new(vec![Message::user(
                "no native observation".into(),
            )]))
            .await
        {
            Err(error) => error,
            Ok(stream) => {
                let items = stream.collect::<Vec<_>>().await;
                assert!(
                    items
                        .iter()
                        .filter_map(|item| item.as_ref().ok())
                        .all(|chunk| chunk.finish_reason.is_none()
                            && chunk.usage.is_none()
                            && chunk.delta.provider_response.is_none())
                );
                items
                    .into_iter()
                    .find_map(Result::err)
                    .ok_or(LlmError::EmptyResponse)?
            }
        };
        assert!(
            matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::InvalidResponse(_)))
        );
        let _captured = compat_captured(handle).await?;
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_real_errors_after_metadata_only_keep_first_cause() -> Result<()> {
    let native = json!({"id":"native-before-error","model":"wire-model","choices":[],"usage":{"native_extra":{"nullable":null}}});
    for ending in ["provider", "json", "field-type"] {
        let mut body = compat_sse(std::slice::from_ref(&native), false);
        match ending {
            "provider"=>body.push_str(&compat_sse(&[json!({"error":{"type":"overloaded_error","code":37,"message":"actual provider failure"}})],true)),
            "json"=>body.push_str("data: {broken-json}\n\n"),
            _=>body.push_str(&compat_sse(&[json!({"id":"native-before-error","choices":[],"usage":37})],true)),
        }
        let (url, handle) = compat_server(vec![(200, "text/event-stream".into(), body)]).await?;
        let items = compat_client(&url)?
            .chat_stream(ChatRequest::new(vec![Message::user("first cause".into())]))
            .await?
            .collect::<Vec<_>>()
            .await;
        let error = items
            .iter()
            .find_map(|item| item.as_ref().err())
            .ok_or(LlmError::EmptyResponse)?;
        if ending == "provider" {
            assert!(
                matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::ProviderError{retryable:false,code:Some(code),..} if code==&json!(37)))
            );
        } else {
            assert!(
                matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::InvalidResponse(_)))
            );
        }
        assert!(
            items
                .iter()
                .filter_map(|item| item.as_ref().ok())
                .all(|chunk| chunk.finish_reason.is_none()
                    && chunk.usage.is_none()
                    && chunk.delta.tool_calls.is_none())
        );
        let _captured = compat_captured(handle).await?;
    }
    Ok(())
}

#[tokio::test]
async fn chat_metadata_d2_network_and_cancel_after_metadata_only_keep_original_errors() -> Result<()>
{
    for cancel in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let captured = resume_read_http_body(&mut socket).await?;
            let prefix = compat_sse(
                &[
                    json!({"id":"native-network","choices":[],"model":"wire-model","usage":{"native_extra":{"null":null}}}),
                ],
                false,
            );
            let header = if cancel {
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
                    .to_string()
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    prefix.len().saturating_add(1000)
                )
            };
            socket.write_all(header.as_bytes()).await?;
            socket.write_all(prefix.as_bytes()).await?;
            socket.flush().await?;
            release_rx
                .await
                .map_err(|error| LlmError::NetworkError(error.to_string()))?;
            socket.shutdown().await?;
            Ok::<Value, ReactError>(captured)
        });
        let token = tokio_util::sync::CancellationToken::new();
        let client = compat_client(&url)?;
        let mut request = ChatRequest::new(vec![Message::user("actual transport cause".into())]);
        request.cancel_token = Some(token.clone());
        let mut stream = client.chat_stream(request).await?;
        let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))?
            .ok_or(LlmError::EmptyResponse)??;
        assert!(
            first.delta.provider_response.is_some()
                && first.finish_reason.is_none()
                && first.usage.is_none()
        );
        if cancel {
            token.cancel();
        }
        release_tx
            .send(())
            .map_err(|_| LlmError::NetworkError("fixture release closed".into()))?;
        let rest = tokio::time::timeout(Duration::from_secs(2), stream.collect::<Vec<_>>())
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))?;
        let error = rest
            .iter()
            .find_map(|item| item.as_ref().err())
            .ok_or(LlmError::EmptyResponse)?;
        assert!(
            matches!(error,ReactError::Llm(error) if matches!(error.as_ref(),LlmError::NetworkError(_)))
        );
        if cancel {
            assert!(error.to_string().contains("cancelled"));
        }
        assert!(
            rest.iter()
                .filter_map(|item| item.as_ref().ok())
                .all(|chunk| chunk.finish_reason.is_none()
                    && chunk.usage.is_none()
                    && chunk.delta.tool_calls.is_none())
        );
        let _captured = server
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))??;
    }
    Ok(())
}
