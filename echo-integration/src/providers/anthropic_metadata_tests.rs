//! D1/D2准确共享类型是编译前置；本模块没有新Core类型或第二执行器。

use super::*;
use serde_json::{Value, json};

fn assert_invalid_response(error: echo_core::error::ReactError) {
    assert!(
        matches!(error,echo_core::error::ReactError::Llm(inner) if matches!(inner.as_ref(),LlmError::InvalidResponse(_))),
        "malformed native fields must keep parsing first cause"
    );
}

#[test]
fn metadata_truncated_bad_usage_keeps_parse_first_cause_in_both_modes() -> Result<()> {
    let client = AnthropicClient::new("fixture-only", "test-model");
    for stop in [
        "max_tokens",
        "model_context_window_exceeded",
        "vendor_future_stop",
    ] {
        for malformed in [json!("bad-counter"), json!(4294967296_u64), Value::Null] {
            let raw = json!({"id":"bad-usage","content":[{"type":"text","text":"partial"}],"stop_reason":stop,"usage":{"input_tokens":2,"output_tokens":malformed}});
            let response = serde_json::from_value(raw)
                .map_err(|error| invalid(error.to_string()).into())
                .and_then(|response| client.convert_response(response));
            let error = response
                .err()
                .ok_or_else(|| invalid("malformed full usage was accepted"))?;
            assert_invalid_response(error);
            let mut adapter = NativeAnthropicStream {
                provider: "configured".into(),
                ..Default::default()
            };
            adapter.map(json!({"type":"message_start","message":{"id":"bad-usage","usage":{"input_tokens":2,"output_tokens":0}}}))?;
            adapter.map(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"partial"}}))?;
            adapter.map(json!({"type":"content_block_stop","index":0}))?;
            let error=adapter.map(json!({"type":"message_delta","delta":{"stop_reason":stop},"usage":{"output_tokens":malformed}})).err().ok_or_else(||invalid("malformed delta usage was accepted"))?;
            assert_invalid_response(error);
        }
    }
    let mut adapter = NativeAnthropicStream {
        provider: "configured".into(),
        ..Default::default()
    };
    adapter.map(json!({"type":"message_start","message":{"id":"optional-null","usage":{"input_tokens":2,"output_tokens":0}}}))?;
    let chunks=adapter.map(json!({"type":"message_delta","delta":{},"usage":{"input_tokens":null,"cache_read_input_tokens":null,"output_tokens":7}}))?;
    assert!(
        chunks
            .last()
            .and_then(|chunk| chunk.delta.provider_response.as_ref())
            .and_then(|observation| observation.usage.as_ref())
            .is_some_and(|usage| usage.get("input_tokens") == Some(&Value::Null)
                && usage.get("output_tokens") == Some(&json!(7)))
    );
    Ok(())
}

#[tokio::test]
async fn metadata_malformed_wrapper_or_block_never_becomes_valid_partial() -> Result<()> {
    for start in [Value::Null, json!(17), json!([]), json!("message")] {
        let mut adapter = NativeAnthropicStream {
            provider: "configured".into(),
            ..Default::default()
        };
        let error = adapter
            .map(json!({"type":"message_start","message":start}))
            .err()
            .ok_or_else(|| invalid("malformed message_start was accepted"))?;
        assert_invalid_response(error);
        assert!(
            adapter
                .map(json!({"type":"message_start","message":{"id":"valid-after-rejection"}}))
                .is_ok(),
            "bad wrapper must not install a fake request owner"
        );
    }
    for block in [
        json!({"type":"text","text":null}),
        json!({"type":"text","text":false}),
        json!({"type":"thinking","thinking":7}),
        json!({"type":"thinking","thinking":"","signature":null}),
        json!({"type":"tool_use","id":"call","name":"lookup","input":[]}),
        json!({"type":"tool_use","id":"","name":"lookup","input":{}}),
    ] {
        let native_events = [
            json!({"type":"message_start","message":{"id":"malformed-block"}}),
            json!({"type":"content_block_start","index":0,"content_block":block}),
        ];
        let mut stream = adapt_anthropic_stream(
            futures::stream::iter(
                native_events
                    .into_iter()
                    .map(|event| Ok(JsonSseEvent::Data(event))),
            ),
            "configured".into(),
            false,
        );
        let mut error = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => assert!(chunk.finish_reason.is_none()),
                Err(failure) => error = Some(failure),
            }
        }
        assert_invalid_response(error.ok_or_else(|| invalid("bad block was accepted at EOF"))?);
    }
    Ok(())
}

#[tokio::test]
async fn metadata_deepseek_active_unsigned_partial_keeps_signature_presence() -> Result<()> {
    for (signature, truncated, empty_delta) in [
        (None, false, false),
        (Some(""), false, false),
        (None, true, false),
        (Some(""), true, false),
        (None, false, true),
    ] {
        let mut thinking = json!({"type":"thinking","thinking":""});
        if let Some(signature) = signature {
            thinking
                .as_object_mut()
                .ok_or_else(|| invalid("thinking fixture missing"))?
                .insert("signature".into(), json!(signature));
        }
        let mut native_events = vec![
            json!({"type":"message_start","message":{"id":"unsigned-partial","model":"deepseek-v4-pro","usage":{"input_tokens":2,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":thinking}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"unsigned-原文😀"}}),
        ];
        if empty_delta {
            native_events.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":""}}));
        }
        if truncated {
            native_events.push(json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":3}}));
        }
        let mut stream = adapt_anthropic_stream(
            futures::stream::iter(
                native_events
                    .into_iter()
                    .map(|event| Ok(JsonSseEvent::Data(event))),
            ),
            "configured-deepseek".into(),
            true,
        );
        let mut failure = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => assert!(chunk.finish_reason.is_none()),
                Err(error) => failure = Some(error),
            }
        }
        let partial = decode_partial(
            failure.ok_or_else(|| invalid("active unsigned partial was accepted"))?,
        )?;
        assert!(partial.execution_status.is_none() && partial.approved_assistant_blocks.is_none());
        let blocks = partial
            .reasoning_blocks
            .as_ref()
            .ok_or_else(|| invalid("partial unsigned reasoning missing"))?;
        let data = blocks
            .iter()
            .find_map(|block| match block {
                ReasoningBlock::Opaque { provider, data, .. }
                    if provider == "configured-deepseek" =>
                {
                    Some(data)
                }
                _ => None,
            })
            .ok_or_else(|| invalid("unsigned partial became Signed"))?;
        let raw: Value = serde_json::from_str(data).map_err(|error| invalid(error.to_string()))?;
        assert_eq!(raw.get("thinking"), Some(&json!("unsigned-原文😀")));
        assert_eq!(
            raw.get("signature").cloned(),
            if signature.is_some() || empty_delta {
                Some(json!(""))
            } else {
                None
            }
        );
    }
    Ok(())
}

fn invalid(reason: impl Into<String>) -> LlmError {
    LlmError::InvalidResponse(reason.into())
}

fn cases() -> Result<Vec<Value>> {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../fixtures/providers/anthropic/anthropic-provider-response.json"
    ))
    .map_err(|error| invalid(error.to_string()))?;
    fixture
        .get("cases")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| invalid("metadata cases missing").into())
}

fn message_json(message: &Message) -> Result<Value> {
    serde_json::to_value(message)
        .map_err(|error| LlmError::SerializationError(error.to_string()).into())
}

fn assert_observation(raw: &Value, message: &Message, provider: &str) -> Result<()> {
    let value = message_json(message)?;
    let observation = value
        .get("provider_response")
        .ok_or_else(|| invalid("response observation missing"))?;
    assert_eq!(observation.get("provider"), Some(&json!(provider)));
    assert_eq!(
        observation.get("request_id").cloned(),
        raw.get("id").filter(|value| !value.is_null()).cloned()
    );
    for field in ["model", "stop_reason", "stop_sequence", "usage"] {
        assert_eq!(
            observation.get(field),
            raw.get(field),
            "native presence/value changed: {field}"
        );
    }
    assert!(message.execution_status.is_none());
    assert!(message.approved_assistant_blocks.is_none());
    Ok(())
}

fn decode_partial(error: echo_core::error::ReactError) -> Result<Message> {
    match error {
        echo_core::error::ReactError::Llm(error) => match *error {
            LlmError::IncompleteResponse { partial } => Ok(*partial),
            other => Err(other.into()),
        },
        other => Err(other),
    }
}

fn events(raw: &Value) -> Result<Vec<Value>> {
    let mut events = super::compatibility_tests::sse_events(raw)?;
    let mut start = raw.clone();
    let fields = start
        .as_object_mut()
        .ok_or_else(|| invalid("response fixture must be object"))?;
    fields.insert("content".into(), json!([]));
    for field in ["stop_reason", "stop_sequence"] {
        if fields.contains_key(field) {
            fields.insert(field.into(), Value::Null);
        }
    }
    if let Some(usage) = fields.get_mut("usage").and_then(Value::as_object_mut)
        && usage.contains_key("output_tokens")
    {
        usage.insert("output_tokens".into(), json!(0));
    }
    let first = events
        .first_mut()
        .ok_or_else(|| invalid("message_start fixture missing"))?;
    *first = json!({"type":"message_start","message":start});
    let terminal = events
        .iter_mut()
        .find(|event| event.get("type").and_then(Value::as_str) == Some("message_delta"))
        .ok_or_else(|| invalid("message_delta fixture missing"))?;
    let mut delta = serde_json::Map::new();
    for field in ["stop_reason", "stop_sequence"] {
        if let Some(value) = raw.get(field) {
            delta.insert(field.into(), value.clone());
        }
    }
    let mut event = json!({"type":"message_delta","delta":delta});
    if let Some(usage) = raw.get("usage") {
        event
            .as_object_mut()
            .ok_or_else(|| invalid("event object missing"))?
            .insert("usage".into(), usage.clone());
    }
    *terminal = event;
    Ok(events)
}

#[test]
fn metadata_nonstream_cases_preserve_presence_unknown_usage_and_incomplete_partial() -> Result<()> {
    let client = AnthropicClient::new("fixture-only", "test-model");
    for case in cases()? {
        let raw = case
            .get("wire")
            .cloned()
            .ok_or_else(|| invalid("native case body missing"))?;
        let response = client.convert_response(
            serde_json::from_value(raw.clone()).map_err(|error| invalid(error.to_string()))?,
        );
        let boundary = case
            .get("expected_boundary")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("boundary missing"))?;
        let message = if boundary == "incomplete_failure" {
            let error = response
                .err()
                .ok_or_else(|| invalid("incomplete response became success"))?;
            let failure = echo_core::error::AgentFailure::try_from(&error)?;
            assert!(!failure.retryable);
            assert_eq!(failure.http_status, None);
            assert!(!error.to_string().contains("公开前文"));
            decode_partial(error)?
        } else {
            let response = response?;
            assert_eq!(
                response.finish_reason.as_deref(),
                Some(if boundary == "continue" {
                    "continue"
                } else {
                    "stop"
                })
            );
            response.message
        };
        assert_observation(&raw, &message, "anthropic")?;
        let cold = super::compatibility_tests::reload(&message)?;
        assert_eq!(
            message_json(&cold)?.get("provider_response"),
            message_json(&message)?.get("provider_response")
        );
    }
    Ok(())
}

#[tokio::test]
async fn metadata_stream_cases_keep_full_snapshots_and_never_success_on_incomplete() -> Result<()> {
    for case in cases()? {
        let raw = case
            .get("wire")
            .cloned()
            .ok_or_else(|| invalid("native case body missing"))?;
        let boundary = case
            .get("expected_boundary")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("boundary missing"))?;
        let native_events = events(&raw)?
            .into_iter()
            .map(|event| Ok(JsonSseEvent::Data(event)));
        let mut stream = adapt_anthropic_stream(
            futures::stream::iter(native_events),
            "configured-service".into(),
            false,
        );
        let mut finish = None;
        let mut last_observation = None;
        let mut failure = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => {
                    if chunk.delta.provider_response.is_some() {
                        last_observation = chunk.delta.provider_response;
                    }
                    if chunk.finish_reason.is_some() {
                        assert!(finish.is_none());
                        finish = chunk.finish_reason;
                    }
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        if boundary == "incomplete_failure" {
            assert!(
                finish.is_none(),
                "partial must never publish success terminal"
            );
            let partial =
                decode_partial(failure.ok_or_else(|| invalid("incomplete stream omitted error"))?)?;
            assert_observation(&raw, &partial, "configured-service")?;
        } else {
            assert!(failure.is_none());
            assert_eq!(
                finish.as_deref(),
                Some(if boundary == "continue" {
                    "continue"
                } else {
                    "stop"
                })
            );
            let message = Message {
                provider_response: last_observation,
                ..Default::default()
            };
            assert_observation(&raw, &message, "configured-service")?;
        }
    }
    Ok(())
}

#[test]
fn metadata_usage_only_updates_are_full_snapshots_without_double_count_or_request_bleed()
-> Result<()> {
    let mut adapter = NativeAnthropicStream {
        provider: "actual-config".into(),
        ..Default::default()
    };
    adapter.map(json!({"type":"message_start","message":{"id":"request-A","model":null,"usage":{"input_tokens":11,"output_tokens":0,"cache_read_input_tokens":3,"nested":{"retained":"原文","other":false}}}}))?;
    let mut last_observation = None;
    for _ in 0..2 {
        let chunks = adapter.map(json!({"type":"message_delta","delta":{},"usage":{"output_tokens":7,"nested":{"added":null}}}))?;
        let snapshot = chunks
            .last()
            .and_then(|chunk| chunk.delta.provider_response.as_ref())
            .ok_or_else(|| invalid("usage snapshot missing"))?;
        assert_eq!(snapshot.request_id.as_deref(), Some("request-A"));
        assert_eq!(snapshot.model, Some(Value::Null));
        assert_eq!(
            snapshot
                .usage
                .as_ref()
                .and_then(|usage| usage.get("output_tokens")),
            Some(&json!(7))
        );
        assert_eq!(
            snapshot
                .usage
                .as_ref()
                .and_then(|usage| usage.pointer("/nested/retained")),
            Some(&json!("原文"))
        );
        assert_eq!(
            snapshot
                .usage
                .as_ref()
                .and_then(|usage| usage.pointer("/nested/added")),
            Some(&Value::Null)
        );
        assert!(chunks.iter().all(|chunk| chunk.finish_reason.is_none()));
        last_observation = Some(snapshot.clone());
    }
    let before = last_observation;
    assert!(adapter.map(json!({"type":"message_start","message":{"id":"request-B","usage":{"input_tokens":999}}})).is_err());
    let after = adapter.map(json!({"type":"message_delta","delta":{},"usage":{}}))?;
    assert_eq!(
        after
            .last()
            .and_then(|chunk| chunk.delta.provider_response.clone()),
        before
    );
    let mut other = NativeAnthropicStream {
        provider: "actual-config".into(),
        ..Default::default()
    };
    let other_chunks = other.map(json!({"type":"message_start","message":{"id":"request-B"}}))?;
    assert!(
        other_chunks
            .last()
            .and_then(|chunk| chunk.delta.provider_response.as_ref())
            .is_some_and(|metadata| metadata.usage.is_none() && metadata.model.is_none())
    );
    Ok(())
}

#[tokio::test]
async fn metadata_clean_eof_keeps_partial_without_retry_and_provider_errors_stay_typed()
-> Result<()> {
    let raw = json!({"id":"partial-request","model":"actual-service-model","content":[{"type":"thinking","thinking":"signed content","signature":"signature-original"},{"type":"text","text":"PRIVATE-PARTIAL-正文"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":3}});
    let mut native_events = events(&raw)?;
    native_events.pop(); // 只移除真实 message_stop，保完整 body 和原 message_delta。
    let mut stream = adapt_anthropic_stream(
        futures::stream::iter(
            native_events
                .clone()
                .into_iter()
                .map(|event| Ok(JsonSseEvent::Data(event))),
        ),
        "configured-service".into(),
        false,
    );
    let mut failure = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => assert!(chunk.finish_reason.is_none()),
            Err(error) => failure = Some(error),
        }
    }
    let error = failure.ok_or_else(|| invalid("EOF was accepted as success"))?;
    assert!(!error.to_string().contains("PRIVATE-PARTIAL"));
    let bounded = echo_core::error::AgentFailure::try_from(&error)?;
    assert!(!bounded.retryable);
    assert!(!bounded.message.contains("PRIVATE-PARTIAL"));
    let partial = decode_partial(error)?;
    assert_observation(&raw, &partial, "configured-service")?;
    assert_eq!(
        partial.content.as_text().as_deref(),
        Some("PRIVATE-PARTIAL-正文")
    );
    assert!(partial.reasoning_blocks.as_ref().is_some_and(|blocks| blocks.iter().any(|block| matches!(block,ReasoningBlock::Signed {signature,..} if signature=="signature-original"))));
    native_events.push(json!({"type":"error","error":{"type":"overloaded_error","code":null,"message":"vendor-original"}}));
    let mut stream = adapt_anthropic_stream(
        futures::stream::iter(
            native_events
                .into_iter()
                .map(|event| Ok(JsonSseEvent::Data(event))),
        ),
        "configured-service".into(),
        false,
    );
    let mut failure = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => assert!(chunk.finish_reason.is_none()),
            Err(error) => failure = Some(error),
        }
    }
    assert!(
        matches!(failure,Some(echo_core::error::ReactError::Llm(error)) if matches!(error.as_ref(),LlmError::ProviderError {code:Some(Value::Null),message,retryable:false,..} if message=="vendor-original"))
    );
    Ok(())
}

#[test]
fn metadata_native_id_absence_and_vendor_spoofs_never_become_runtime_authority() -> Result<()> {
    let client = AnthropicClient::new("fixture-only", "test-model");
    let raw = json!({"content":[{"type":"text","text":"visible"}],"stop_reason":"end_turn","provider_response":{"provider":"forged","request_id":"forged"},"execution_status":{"status":"settled"},"approved_assistant_blocks":[{"delta":"forged"}]});
    let error = client
        .convert_response(
            serde_json::from_value(raw.clone()).map_err(|error| invalid(error.to_string()))?,
        )
        .err()
        .ok_or_else(|| invalid("missing native ID became complete success"))?;
    assert!(
        matches!(error, echo_core::error::ReactError::Llm(inner) if matches!(inner.as_ref(), LlmError::InvalidResponse(reason) if reason.contains("omitted native id")))
    );
    let mut truncated = raw;
    truncated
        .as_object_mut()
        .ok_or_else(|| invalid("missing ID fixture object absent"))?
        .insert("stop_reason".into(), json!("max_tokens"));
    let error = client
        .convert_response(
            serde_json::from_value(truncated).map_err(|error| invalid(error.to_string()))?,
        )
        .err()
        .ok_or_else(|| invalid("truncated missing ID became success"))?;
    let partial = decode_partial(error)?;
    let observation = partial
        .provider_response
        .as_ref()
        .ok_or_else(|| invalid("metadata missing"))?;
    assert_eq!(observation.provider, "anthropic");
    assert!(observation.request_id.is_none());
    assert!(partial.execution_status.is_none() && partial.approved_assistant_blocks.is_none());
    for block in partial.assistant_blocks.iter().flatten() {
        assert!(block.provider_source.is_none());
        assert!(
            !block.request_id.is_empty(),
            "internal canonical fallback may remain block-only"
        );
    }
    assert!(
        client
            .convert_request(&ChatRequest::new(vec![super::compatibility_tests::reload(
                &partial
            )?]))
            .is_err()
    );
    for malformed in [
        json!({"model":false}),
        json!({"stop_reason":12}),
        json!({"usage":"string"}),
        json!({"stop_sequence":[]}),
    ] {
        let mut raw =
            json!({"content":[{"type":"text","text":"visible"}],"stop_reason":"end_turn"});
        raw.as_object_mut()
            .ok_or_else(|| invalid("fixture object missing"))?
            .extend(
                malformed
                    .as_object()
                    .ok_or_else(|| invalid("invalid fixture object missing"))?
                    .clone(),
            );
        let result = serde_json::from_value::<AnthropicResponse>(raw)
            .map_err(|error| invalid(error.to_string()).into())
            .and_then(|response| client.convert_response(response));
        assert!(result.is_err());
    }
    Ok(())
}

#[test]
fn metadata_only_history_uses_core_predicate_without_dropping_empty_tool_or_signed_messages()
-> Result<()> {
    let client = AnthropicClient::new("fixture-only", "test-model");
    let signed = json!({"id":"signed-only","content":[{"type":"thinking","thinking":"native original","signature":"native-signature"}],"stop_reason":"pause_turn","model":null});
    let signed = client
        .convert_response(
            serde_json::from_value(signed).map_err(|error| invalid(error.to_string()))?,
        )?
        .message;
    let observation = signed
        .provider_response
        .clone()
        .ok_or_else(|| invalid("signed observation missing"))?;
    let record = Message::provider_response_observation(observation);
    assert!(record.is_provider_response_observation_only());
    assert!(!signed.is_provider_response_observation_only());
    let only_record = serde_json::to_value(client.convert_request(&ChatRequest::new(vec![
        Message::user("request".into()),
        record.clone(),
    ]))?)
    .map_err(|error| invalid(error.to_string()))?;
    assert_eq!(
        only_record
            .get("messages")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    let signed_wire = serde_json::to_value(
        client.convert_request(&ChatRequest::new(vec![record.clone(), signed]))?,
    )
    .map_err(|error| invalid(error.to_string()))?;
    assert_eq!(
        signed_wire.pointer("/messages/0/content/0/signature"),
        Some(&json!("native-signature"))
    );
    let tool = json!({"id":"tool-only","content":[{"type":"tool_use","id":"tool-call","name":"lookup","input":{"provider_response":{"legitimate_user_argument":"preserved"}}}],"stop_reason":"tool_use"});
    let tool = client
        .convert_response(
            serde_json::from_value(tool).map_err(|error| invalid(error.to_string()))?,
        )?
        .message;
    assert!(!tool.is_provider_response_observation_only());
    let tool_wire =
        serde_json::to_value(client.convert_request(&ChatRequest::new(vec![record, tool]))?)
            .map_err(|error| invalid(error.to_string()))?;
    assert_eq!(
        tool_wire.pointer("/messages/0/content/0/type"),
        Some(&json!("tool_use"))
    );
    assert_eq!(
        tool_wire.pointer("/messages/0/content/0/input/provider_response/legitimate_user_argument"),
        Some(&json!("preserved"))
    );
    Ok(())
}

#[tokio::test]
async fn metadata_observed_partial_does_not_replace_network_first_cause_with_incomplete()
-> Result<()> {
    let raw_stream = futures::stream::iter(vec![
        Ok(JsonSseEvent::Data(
            json!({"type":"message_start","message":{"id":"net-first","model":"native-model"}}),
        )),
        Ok(JsonSseEvent::Data(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"partial"}}),
        )),
        Err(LlmError::NetworkError("network-first-cause".into()).into()),
    ]);
    let mut stream = adapt_anthropic_stream(raw_stream, "configured-service".into(), false);
    let mut failure = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => assert!(chunk.finish_reason.is_none()),
            Err(error) => failure = Some(error),
        }
    }
    assert!(
        matches!(failure,Some(echo_core::error::ReactError::Llm(error)) if matches!(error.as_ref(),LlmError::NetworkError(message) if message=="network-first-cause"))
    );
    Ok(())
}

#[tokio::test]
async fn metadata_open_tool_eof_keeps_observed_arguments_but_cannot_replay_as_complete_history()
-> Result<()> {
    let native_events = vec![
        json!({"type":"message_start","message":{"id":"open-tool","model":"actual-native-model","stop_reason":null,"usage":{"input_tokens":2,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"thought","signature":"original-signature"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"unexecuted-call","name":"lookup","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
    ];
    let mut stream = adapt_anthropic_stream(
        futures::stream::iter(
            native_events
                .into_iter()
                .map(|event| Ok(JsonSseEvent::Data(event))),
        ),
        "anthropic".into(),
        false,
    );
    let mut failure = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => assert!(chunk.finish_reason.is_none()),
            Err(error) => failure = Some(error),
        }
    }
    let partial = decode_partial(failure.ok_or_else(|| invalid("open tool EOF did not fail"))?)?;
    assert!(partial.execution_status.is_none() && partial.approved_assistant_blocks.is_none());
    assert_eq!(
        partial
            .tool_calls
            .as_ref()
            .and_then(|calls| calls.first())
            .map(|call| call.function.arguments.as_str()),
        Some("{\"path\":")
    );
    assert!(
        partial
            .provider_response
            .as_ref()
            .is_some_and(
                |observation| observation.request_id.as_deref() == Some("open-tool")
                    && observation.stop_reason == Some(Value::Null)
            )
    );
    assert!(partial.reasoning_blocks.as_ref().is_some_and(|blocks|blocks.iter().any(|block|matches!(block,ReasoningBlock::Signed{signature,..} if signature=="original-signature"))));
    let client = AnthropicClient::new("fixture-only", "test-model");
    assert!(
        client
            .convert_request(&ChatRequest::new(vec![partial]))
            .is_err(),
        "partial tool JSON must not become a valid next-model history"
    );
    Ok(())
}

#[tokio::test]
async fn metadata_deepseek_unsigned_keeps_real_configured_identity_and_replay() -> Result<()> {
    let client = AnthropicClient::with_base_url(
        "https://api.deepseek.com/anthropic/v1/messages",
        "fixture-only",
        "claude-opus-4-5",
    )
    .with_model_profile_resolver("configured-deepseek", ModelProfileResolver::new());
    let raw = json!({"id":"deepseek-meta","model":"deepseek-v4-pro","stop_reason":"end_turn","content":[{"type":"thinking","thinking":"unsigned-original"},{"type":"text","text":"visible"}],"usage":{"input_tokens":2,"output_tokens":3,"future":{"value":null}}});
    let response = client.convert_response(
        serde_json::from_value(raw.clone()).map_err(|error| invalid(error.to_string()))?,
    )?;
    assert_observation(&raw, &response.message, "configured-deepseek")?;
    let request =
        client.convert_request(&ChatRequest::new(vec![super::compatibility_tests::reload(
            &response.message,
        )?]))?;
    let wire = serde_json::to_value(request).map_err(|error| invalid(error.to_string()))?;
    assert!(wire.pointer("/messages/0/provider_response").is_none());
    assert_eq!(
        wire.pointer("/messages/0/content/0/thinking"),
        Some(&json!("unsigned-original"))
    );
    assert!(wire.pointer("/messages/0/content/0/signature").is_none());
    let mut stream = adapt_anthropic_stream(
        futures::stream::iter(
            events(&raw)?
                .into_iter()
                .map(|event| Ok(JsonSseEvent::Data(event))),
        ),
        "configured-deepseek".into(),
        true,
    );
    let mut last = None;
    let mut unsigned = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if chunk.delta.provider_response.is_some() {
            last = chunk.delta.provider_response;
        }
        unsigned|=chunk.delta.reasoning_blocks.iter().flatten().any(|reasoning|matches!(reasoning,ReasoningBlock::Opaque{provider,..} if provider=="configured-deepseek"));
    }
    assert!(unsigned);
    let message = Message {
        provider_response: last,
        ..Default::default()
    };
    assert_observation(&raw, &message, "configured-deepseek")?;
    Ok(())
}

#[tokio::test]
async fn metadata_http_cold_serde_then_next_request_strips_observation_and_keeps_native_order()
-> Result<()> {
    use super::compatibility_tests::{
        collect_message, encode_events, read_request, reload, request, rich_result, write_response,
    };
    use std::time::Duration;
    use tokio::net::TcpListener;
    for streaming in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let mut rounds = super::compatibility_tests::fixture_rounds()?;
        let first = rounds
            .first_mut()
            .ok_or_else(|| invalid("signed round missing"))?;
        first
            .as_object_mut()
            .ok_or_else(|| invalid("round object missing"))?
            .insert("stop_sequence".into(), Value::Null);
        first
            .as_object_mut()
            .ok_or_else(|| invalid("round object missing"))?
            .insert("model".into(), json!("actual-native-model"));
        let first_raw = first.clone();
        let final_raw = json!({"id":"final-meta","content":[{"type":"text","text":"done"}],"stop_reason":"stop_sequence","stop_sequence":"STOP-中文","model":null,"usage":{"input_tokens":17,"output_tokens":3,"nested":{"extra":null}}});
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for raw in [first_raw, final_raw] {
                let (mut socket, _) = listener.accept().await?;
                requests.push(read_request(&mut socket).await?);
                if streaming {
                    write_response(
                        &mut socket,
                        "text/event-stream",
                        &encode_events(&events(&raw)?),
                    )
                    .await?;
                } else {
                    write_response(&mut socket, "application/json", &raw.to_string()).await?;
                }
            }
            Ok::<_, echo_core::error::ReactError>(requests)
        });
        let client = AnthropicClient::with_base_url(
            format!("http://{address}"),
            "fixture-only",
            "test-model",
        );
        let result = async {
            let mut request = request(None);
            let (first, stop, _) = collect_message(&client, request.clone(), streaming).await?;
            assert_eq!(stop.as_deref(), Some("tool_calls"));
            assert_observation(
                rounds.first().ok_or_else(|| invalid("round missing"))?,
                &first,
                "anthropic",
            )?;
            let first = reload(&first)?;
            request.messages.push(first);
            request.messages.push(rich_result("call_a"));
            request.messages.push(Message {
                role: Role::Tool,
                tool_call_id: Some("call_b".into()),
                content: MessageContent::Text("second".into()),
                ..Default::default()
            });
            let (last, stop, _) = collect_message(&client, request, streaming).await?;
            assert_eq!(stop.as_deref(), Some("stop"));
            assert!(last.provider_response.as_ref().is_some_and(|metadata| {
                metadata.request_id.as_deref() == Some("final-meta")
                    && metadata.model == Some(Value::Null)
            }));
            Ok::<_, echo_core::error::ReactError>(())
        }
        .await;
        if let Err(error) = result {
            server.abort();
            let _settled = server.await;
            return Err(error);
        }
        let captured = tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .map_err(|_| invalid("metadata HTTP fixture did not settle"))?
            .map_err(|error| invalid(error.to_string()))??;
        let second = captured
            .get(1)
            .ok_or_else(|| invalid("next request missing"))?;
        fn contains_observation(value: &Value) -> bool {
            match value {
                Value::Object(fields) => fields
                    .iter()
                    .any(|(key, value)| key == "provider_response" || contains_observation(value)),
                Value::Array(items) => items.iter().any(contains_observation),
                _ => false,
            }
        }
        assert!(!contains_observation(second));
        let sent = second
            .pointer("/messages/1/content")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("native history missing"))?;
        let original = rounds
            .first()
            .and_then(|raw| raw.get("content"))
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("original history missing"))?;
        assert_eq!(sent.len(), original.len());
        for (sent, original) in sent.iter().zip(original) {
            let mut sent = sent.clone();
            if let Some(fields) = sent.as_object_mut() {
                fields.remove("cache_control");
            }
            assert_eq!(&sent, original);
        }
        assert_eq!(
            second.pointer("/messages/2/content/0/content/1/type"),
            Some(&json!("image"))
        );
        assert_eq!(
            second.pointer("/messages/2/content/0/content/2/type"),
            Some(&json!("document"))
        );
    }
    Ok(())
}

#[tokio::test]
async fn metadata_http_observation_only_eof_keeps_partial_and_original_first_causes() -> Result<()>
{
    use super::compatibility_tests::{encode_events, read_request, write_response};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::net::TcpListener;

    let native = json!({"id":"metadata-only-中文","model":"native-model","stop_reason":null,"stop_sequence":null,"content":[],"usage":{"input_tokens":7,"output_tokens":0,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"future":{"present":null,"large":18446744073709551615_u64}}});
    let start = encode_events(&[json!({"type":"message_start","message":native.clone()})]);
    let mut cases = Vec::new();
    for raw in [
        native.clone(),
        json!({"model":"native-without-id"}),
        json!({"id":"native-id-only"}),
        json!({"model":null}),
    ] {
        cases.push((
            "partial",
            encode_events(&[json!({"type":"message_start","message":raw.clone()})]),
            Some(raw),
        ));
    }
    cases.extend([
        ("empty", String::new(), None),
        ("done", "data: [DONE]\n\n".into(), None),
        ("done", format!("{start}data: [DONE]\n\n"), None),
        ("wrapper", encode_events(&[json!({"type":"message_start","message":{}})]), None),
        ("json", format!("{start}data: {{broken}}\n\n"), None),
        ("provider", format!("{start}{}", encode_events(&[json!({"type":"error","error":{"type":"overloaded_error","code":null,"message":"provider-first-cause"}})])), None),
    ]);
    for (cause, body, raw) in cases {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let request_received = Arc::new(AtomicBool::new(false));
        let received = request_received.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            read_request(&mut socket).await?;
            received.store(true, Ordering::SeqCst);
            write_response(&mut socket, "text/event-stream", &body).await
        });
        let client = AnthropicClient::with_base_url(
            format!("http://{address}"),
            "fixture-only",
            "test-model",
        );
        let mut chunks = Vec::new();
        let mut failure = None;
        match client
            .chat_stream(ChatRequest::new(vec![Message::user(
                "metadata EOF probe".into(),
            )]))
            .await
        {
            Err(error) => failure = Some(error),
            Ok(mut stream) => {
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(chunk) => chunks.push(chunk),
                        Err(error) => {
                            assert!(failure.is_none(), "fixture must expose one terminal error");
                            failure = Some(error);
                        }
                    }
                }
            }
        }
        if !request_received.load(Ordering::SeqCst) {
            server.abort();
            let _settled = server.await;
            return Err(failure.unwrap_or_else(|| {
                invalid("metadata EOF fixture did not receive production HTTP request").into()
            }));
        }
        server.await.map_err(|error| invalid(error.to_string()))??;
        assert!(
            chunks.iter().all(|chunk| chunk.finish_reason.is_none()
                && chunk.usage.is_none()
                && chunk.delta.tool_calls.is_none()),
            "metadata-only EOF cannot invent completion, usage chunks or tools"
        );
        let failure =
            failure.ok_or_else(|| invalid("metadata EOF fixture completed successfully"))?;
        if let Some(raw) = raw {
            let partial = decode_partial(failure)?;
            assert_observation(&raw, &partial, "anthropic")?;
            assert_eq!(partial.role, Role::Assistant);
            assert!(matches!(partial.content, MessageContent::Empty));
            assert!(
                partial.tool_calls.is_none()
                    && partial.reasoning_content.is_none()
                    && partial.reasoning_blocks.is_none()
                    && partial.assistant_blocks.is_none()
            );
            assert_eq!(
                chunks.len(),
                1,
                "one admitted start must publish one observation"
            );
            assert_eq!(
                chunks
                    .first()
                    .and_then(|chunk| chunk.delta.provider_response.as_ref()),
                partial.provider_response.as_ref()
            );
            if raw == native {
                let mut adapter = NativeAnthropicStream {
                    provider: "anthropic".into(),
                    ..Default::default()
                };
                adapter.map(json!({"type":"message_start","message":raw}))?;
                assert_eq!(adapter.usage.prompt_tokens, Some(7));
                assert_eq!(adapter.usage.completion_tokens, Some(0));
                assert_eq!(adapter.usage.total_tokens, Some(7));
                assert_eq!(adapter.usage.cache_creation_input_tokens, Some(2));
                assert_eq!(adapter.usage.cache_read_input_tokens, Some(3));
            }
        } else if cause == "provider" {
            assert!(
                matches!(failure, echo_core::error::ReactError::Llm(error) if matches!(error.as_ref(), LlmError::ProviderError { provider, error_type, code, message, retryable } if provider == "anthropic" && error_type.as_deref() == Some("overloaded_error") && code == &Some(Value::Null) && message == "provider-first-cause" && !retryable))
            );
        } else {
            match failure {
                echo_core::error::ReactError::Llm(error) => match error.as_ref() {
                    LlmError::InvalidResponse(message) => match cause {
                        "empty" => assert!(message.contains("before the first chunk")),
                        "done" => assert!(message.contains("[DONE] preceded message_stop")),
                        "wrapper" => assert!(message.contains("SSE EOF before message_stop")),
                        "json" => assert!(
                            !message.contains("SSE EOF"),
                            "malformed JSON first cause must survive"
                        ),
                        _ => return Err(invalid("unknown metadata EOF fixture case").into()),
                    },
                    _ => {
                        return Err(invalid(format!(
                            "metadata EOF {cause} changed first cause: {error}"
                        ))
                        .into());
                    }
                },
                other => return Err(other),
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn metadata_http_native_id_provenance_rejects_fallback_success_and_preserves_partial()
-> Result<()> {
    use super::compatibility_tests::{
        collect_message, encode_events, read_request, reload, write_response,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::net::TcpListener;

    let original = json!({"id":"native-provenance","model":"actual-native-model","content":[{"type":"text","text":"before中文"},{"type":"thinking","thinking":"signed-original","signature":"native-signature=="},{"type":"tool_use","id":"native-call","name":"lookup","input":{"path":"中文.md"}},{"type":"redacted_thinking","data":"redacted-original=="},{"type":"text","text":"after😀"}],"stop_reason":"tool_use","usage":{"input_tokens":7,"output_tokens":3}});
    for streaming in [true, false] {
        for scenario in [
            "complete",
            "partial",
            "open_tool",
            "metadata_only",
            "active_reasoning",
            "known",
        ] {
            if !streaming && matches!(scenario, "open_tool" | "metadata_only" | "active_reasoning")
            {
                continue;
            }
            let mut raw = original.clone();
            if scenario != "known" {
                raw.as_object_mut()
                    .ok_or_else(|| invalid("native ID fixture missing object"))?
                    .remove("id");
            }
            if !streaming && scenario == "partial" {
                raw.as_object_mut()
                    .ok_or_else(|| invalid("native ID fixture missing object"))?
                    .insert("stop_reason".into(), json!("max_tokens"));
            }
            let body = if streaming {
                let mut native_events = events(&raw)?;
                if scenario == "partial" {
                    native_events.pop();
                    native_events.pop(); // 保留完整原始块，去掉terminal delta/stop以制造clean EOF。
                } else if scenario == "open_tool" {
                    native_events = vec![
                        json!({"type":"message_start","message":{"model":"actual-native-model","usage":{"input_tokens":7,"output_tokens":0}}}),
                        json!({"type":"content_block_start","index":3,"content_block":{"type":"tool_use","id":"native-call","name":"lookup","input":{}}}),
                        json!({"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"中文"}}),
                    ];
                } else if scenario == "active_reasoning" {
                    native_events = vec![
                        json!({"type":"message_start","message":{"model":"actual-native-model"}}),
                        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"signed-original","signature":"native-signature=="}}),
                        json!({"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"redacted-original=="}}),
                    ];
                } else if scenario == "metadata_only" {
                    native_events = vec![
                        json!({"type":"message_start","message":{"model":"actual-native-model","usage":{"input_tokens":7,"output_tokens":0}}}),
                    ];
                }
                encode_events(&native_events)
            } else {
                raw.to_string()
            };
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let request_received = Arc::new(AtomicBool::new(false));
            let received = request_received.clone();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await?;
                read_request(&mut socket).await?;
                received.store(true, Ordering::SeqCst);
                write_response(
                    &mut socket,
                    if streaming {
                        "text/event-stream"
                    } else {
                        "application/json"
                    },
                    &body,
                )
                .await
            });
            let client = AnthropicClient::with_base_url(
                format!("http://{address}"),
                "fixture-only",
                "test-model",
            );
            let request = ChatRequest::new(vec![Message::user("native ID provenance".into())]);
            let mut message = None;
            let mut failure = None;
            let mut finish = None;
            let mut chunks = Vec::new();
            if scenario == "known" {
                match collect_message(&client, request, streaming).await {
                    Ok((observed, reason, _usage)) => {
                        message = Some(observed);
                        finish = reason;
                    }
                    Err(error) => failure = Some(error),
                }
            } else if streaming {
                match client.chat_stream(request).await {
                    Err(error) => failure = Some(error),
                    Ok(mut stream) => {
                        while let Some(result) = stream.next().await {
                            match result {
                                Ok(chunk) => {
                                    if chunk.finish_reason.is_some() {
                                        finish = chunk.finish_reason.clone();
                                    }
                                    chunks.push(chunk);
                                }
                                Err(error) => {
                                    assert!(failure.is_none());
                                    failure = Some(error);
                                }
                            }
                        }
                    }
                }
            } else {
                match client.chat(request).await {
                    Ok(response) => {
                        message = Some(response.message);
                        finish = response.finish_reason;
                    }
                    Err(error) => failure = Some(error),
                }
            }
            if !request_received.load(Ordering::SeqCst) {
                server.abort();
                let _settled = server.await;
                return Err(failure.unwrap_or_else(|| {
                    invalid("native ID fixture did not receive production HTTP request").into()
                }));
            }
            server.await.map_err(|error| invalid(error.to_string()))??;
            if scenario == "known" {
                if let Some(error) = failure {
                    return Err(error);
                }
                assert_eq!(finish.as_deref(), Some("tool_calls"));
                let observed =
                    reload(&message.ok_or_else(|| invalid("known native ID response missing"))?)?;
                assert_observation(&original, &observed, "anthropic")?;
                for block in observed.assistant_blocks.iter().flatten() {
                    assert_eq!(
                        block
                            .provider_source
                            .as_ref()
                            .map(|source| source.request_id.as_str()),
                        Some("native-provenance")
                    );
                }
                for tool in observed.tool_calls.iter().flatten() {
                    assert_eq!(
                        tool.provider_source
                            .as_ref()
                            .map(|source| source.request_id.as_str()),
                        Some("native-provenance")
                    );
                }
                let wire = serde_json::to_value(
                    client.convert_request(&ChatRequest::new(vec![observed.clone()]))?,
                )
                .map_err(|error| invalid(error.to_string()))?;
                let replayed = wire
                    .pointer("/messages/0/content")
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid("known native replay missing"))?;
                let expected = original
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid("known fixture content missing"))?;
                assert_eq!(replayed.len(), expected.len());
                for (block, expected) in replayed.iter().zip(expected) {
                    let mut block = block.clone();
                    if let Some(object) = block.as_object_mut() {
                        object.remove("cache_control");
                    }
                    assert_eq!(&block, expected, "signed/redacted/tool order changed");
                }
                // 模拟旧错误快照：Source互相一致，但D1没有实际原生ID；不能成功重放。
                let mut forged = observed.clone();
                forged
                    .provider_response
                    .as_mut()
                    .ok_or_else(|| invalid("known observation missing"))?
                    .request_id = None;
                assert!(
                    client
                        .convert_request(&ChatRequest::new(vec![reload(&forged)?]))
                        .is_err()
                );
                for block in forged.assistant_blocks.iter_mut().flatten() {
                    if let Some(source) = block.provider_source.as_mut() {
                        source.request_id.clear();
                    }
                }
                for tool in forged.tool_calls.iter_mut().flatten() {
                    if let Some(source) = tool.provider_source.as_mut() {
                        source.request_id.clear();
                    }
                }
                forged.provider_response = None;
                assert!(
                    client
                        .convert_request(&ChatRequest::new(vec![reload(&forged)?]))
                        .is_err()
                );
                continue;
            }
            for chunk in &chunks {
                for block in chunk.delta.assistant_blocks.iter().flatten() {
                    assert!(
                        block.provider_source.is_none(),
                        "internal fallback cannot become native block Source"
                    );
                    assert!(
                        !block.request_id.is_empty(),
                        "canonical local identity may remain stable"
                    );
                }
                for tool in chunk.delta.tool_calls.iter().flatten() {
                    assert!(
                        tool.provider_source.is_none(),
                        "unknown native ID cannot become live tool Source"
                    );
                }
            }
            assert!(
                finish.is_none(),
                "unknown native ID cannot finish successfully"
            );
            assert!(
                message.is_none(),
                "unknown native ID cannot return a successful response"
            );
            let error = failure.ok_or_else(|| invalid("unknown native ID silently succeeded"))?;
            if scenario == "complete" {
                assert!(
                    matches!(error, echo_core::error::ReactError::Llm(inner) if matches!(inner.as_ref(), LlmError::InvalidResponse(reason) if reason.contains("omitted native id")))
                );
                continue;
            }
            let partial = decode_partial(error)?;
            assert!(
                partial
                    .provider_response
                    .as_ref()
                    .is_some_and(|metadata| metadata.request_id.is_none())
            );
            assert!(
                partial.execution_status.is_none() && partial.approved_assistant_blocks.is_none()
            );
            for block in partial.assistant_blocks.iter().flatten() {
                assert!(block.provider_source.is_none());
            }
            if scenario == "metadata_only" {
                assert!(matches!(partial.content, MessageContent::Empty));
                assert!(partial.assistant_blocks.is_none() && partial.tool_calls.is_none());
            } else if scenario == "active_reasoning" {
                assert!(partial.tool_calls.is_none());
                assert!(partial.reasoning_blocks.iter().flatten().any(|block| matches!(block, ReasoningBlock::Signed {signature,..} if signature == "native-signature==")));
                assert!(partial.reasoning_blocks.iter().flatten().any(|block| matches!(block, ReasoningBlock::Redacted {data} if data == "redacted-original==")));
                assert!(
                    client
                        .convert_request(&ChatRequest::new(vec![reload(&partial)?]))
                        .is_err()
                );
            } else {
                let tool = partial
                    .tool_calls
                    .as_ref()
                    .and_then(|tools| tools.first())
                    .ok_or_else(|| invalid("partial native tool missing"))?;
                let source = tool
                    .provider_source
                    .as_ref()
                    .ok_or_else(|| invalid("partial tool coordinates missing"))?;
                assert!(
                    source.request_id.is_empty(),
                    "D2 unknown native ID must stay unknown"
                );
                assert_eq!(source.item_id, "native-call");
                assert_eq!(
                    source.output_index,
                    if scenario == "open_tool" { 3 } else { 2 }
                );
                assert_eq!(tool.id, "native-call");
                assert_eq!(
                    tool.function.arguments,
                    if scenario == "open_tool" {
                        "{\"path\":\"中文"
                    } else {
                        "{\"path\":\"中文.md\"}"
                    }
                );
                if scenario == "partial" {
                    assert_eq!(
                        partial.content.as_text().as_deref(),
                        Some("before中文after😀")
                    );
                    assert!(partial.reasoning_blocks.iter().flatten().any(|block| matches!(block, ReasoningBlock::Signed {signature,..} if signature == "native-signature==")));
                    assert!(partial.reasoning_blocks.iter().flatten().any(|block| matches!(block, ReasoningBlock::Redacted {data} if data == "redacted-original==")));
                }
                assert!(
                    client
                        .convert_request(&ChatRequest::new(vec![reload(&partial)?]))
                        .is_err()
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn metadata_http_nonstream_native_id_provenance_never_accepts_generated_source() -> Result<()>
{
    use super::compatibility_tests::{read_request, write_response};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let received = Arc::new(AtomicBool::new(false));
    let server_received = received.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        read_request(&mut socket).await?;
        server_received.store(true, Ordering::SeqCst);
        write_response(&mut socket, "application/json", &json!({"model":"actual-model","content":[{"type":"text","text":"without-native-ID"}],"stop_reason":"end_turn"}).to_string()).await
    });
    let client =
        AnthropicClient::with_base_url(format!("http://{address}"), "fixture-only", "test-model");
    let result = client
        .chat(ChatRequest::new(vec![Message::user(
            "native ID probe".into(),
        )]))
        .await;
    if !received.load(Ordering::SeqCst) {
        server.abort();
        let _settled = server.await;
        return Err(result
            .err()
            .unwrap_or_else(|| invalid("nonstream native ID fixture was not reached").into()));
    }
    server.await.map_err(|error| invalid(error.to_string()))??;
    match result {
        Ok(response) => {
            assert!(
                response
                    .message
                    .provider_response
                    .as_ref()
                    .is_some_and(|metadata| metadata.request_id.is_none())
            );
            assert!(
                response
                    .message
                    .assistant_blocks
                    .iter()
                    .flatten()
                    .all(|block| block.provider_source.is_none()),
                "missing native ID was replaced by generated Source"
            );
            Err(invalid("missing native ID produced successful response").into())
        }
        Err(error) => {
            assert!(
                matches!(error, echo_core::error::ReactError::Llm(inner) if matches!(inner.as_ref(), LlmError::InvalidResponse(reason) if reason.contains("omitted native id")))
            );
            Ok(())
        }
    }
}

/// 当前Anthropic producer不升级late ID；这是合法HTTP原件构造的DTO/cold防线，非Root collector验收。
#[tokio::test]
async fn metadata_http_late_source_replay_keeps_prefix_and_signed_or_opaque_fulltext() -> Result<()>
{
    use super::compatibility_tests::{
        collect_message, encode_events, read_request, reload, write_response,
    };
    use std::time::Duration;
    use tokio::net::TcpListener;
    for unsigned in [false, true] {
        for streaming in [false, true] {
            let mut thinking = json!({"type":"thinking","thinking":"完整思考前缀中文😀"});
            if !unsigned {
                thinking
                    .as_object_mut()
                    .ok_or_else(|| invalid("thinking fixture missing"))?
                    .insert("signature".into(), json!("original-signature=="));
            }
            let mut content = vec![
                json!({"type":"text","text":"正文前缀中文😀"}),
                thinking,
                json!({"type":"tool_use","id":"late-call","name":"lookup","input":{"path":"原始.md"}}),
            ];
            if !unsigned {
                content.push(json!({"type":"redacted_thinking","data":"original-redacted=="}));
            }
            content.push(json!({"type":"text","text":"正文后缀"}));
            let original = json!({"id":"actual-late-source","content":content,"stop_reason":"tool_use","usage":{"input_tokens":2,"output_tokens":3}});
            let first_raw = original.clone();
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let server = tokio::spawn(async move {
                let mut captured = Vec::new();
                for raw in [
                    first_raw,
                    json!({"id":"actual-next","content":[{"type":"text","text":"done"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}),
                ] {
                    let (mut socket, _) = listener.accept().await?;
                    captured.push(read_request(&mut socket).await?);
                    if streaming {
                        write_response(
                            &mut socket,
                            "text/event-stream",
                            &encode_events(&events(&raw)?),
                        )
                        .await?;
                    } else {
                        write_response(&mut socket, "application/json", &raw.to_string()).await?;
                    }
                }
                Ok::<_, echo_core::error::ReactError>(captured)
            });
            let mut client = AnthropicClient::with_base_url(
                format!("http://{address}"),
                "fixture-only",
                "test-model",
            );
            if unsigned {
                // 复用既有DeepSeek loopback方式，保留真实方言判定，不伪造producer能力。
                client = AnthropicClient::with_base_url(
                    format!(
                        "http://api.deepseek.com:{}/anthropic/v1/messages",
                        address.port()
                    ),
                    "fixture-only",
                    "deepseek-v4-pro",
                );
                client.client = Arc::new(
                    Client::builder()
                        .no_proxy()
                        .resolve("api.deepseek.com", address)
                        .build()
                        .map_err(|error| invalid(error.to_string()))?,
                );
            }
            let result = async {
                let mut request = ChatRequest::new(vec![Message::user("first actual HTTP".into())]);
                let (mut first, stop, _) =
                    collect_message(&client, request.clone(), streaming).await?;
                if stop.as_deref() != Some("tool_calls") {
                    return Err(invalid("first native response did not complete tool call").into());
                }
                let actual = first
                    .assistant_blocks
                    .clone()
                    .ok_or_else(|| invalid("first HTTP native blocks missing"))?;
                let mut delayed = Vec::new();
                // 显式构造冷恢复DTO：canonical IDs逐字沿真实原件，只有Source延迟到empty closure。
                for (index, block) in actual.iter().enumerate() {
                    let mut prefix = block.clone();
                    prefix.provider_source = None;
                    prefix.reasoning = None;
                    prefix.complete = false;
                    prefix.sequence = u64::try_from(delayed.len())
                        .map_err(|_| invalid("late Source sequence overflow"))?;
                    delayed.push(prefix);
                    if !actual.iter().skip(index.saturating_add(1)).any(|later| {
                        later.request_id == block.request_id && later.block_id == block.block_id
                    }) {
                        let mut closure = block.clone();
                        closure.delta.clear();
                        closure.complete = true;
                        closure.sequence = u64::try_from(delayed.len())
                            .map_err(|_| invalid("late Source sequence overflow"))?;
                        delayed.push(closure);
                    }
                }
                let mut runtime = actual
                    .first()
                    .cloned()
                    .ok_or_else(|| invalid("runtime control template missing"))?;
                runtime.provider_source = None;
                runtime.request_id = "runtime-only".into();
                runtime.block_id = "runtime-only".into();
                runtime.delta = "runtime-only-must-not-be-wire".into();
                runtime.reasoning = None;
                delayed.push(runtime.clone());
                first.assistant_blocks = Some(delayed);
                first.approved_assistant_blocks = Some(vec![runtime]);
                request.messages.push(reload(&first)?);
                request.messages.push(Message::tool_result(
                    "late-call".into(),
                    "lookup".into(),
                    "actual result".into(),
                ));
                request
                    .messages
                    .push(Message::user("next actual HTTP".into()));
                let (_, stop, _) = collect_message(&client, request, streaming).await?;
                if stop.as_deref() != Some("stop") {
                    return Err(invalid("next HTTP response did not complete").into());
                }
                Ok::<_, echo_core::error::ReactError>(())
            }
            .await;
            if let Err(error) = result {
                server.abort();
                let _settled = server.await;
                return Err(error);
            }
            let captured = tokio::time::timeout(Duration::from_secs(3), server)
                .await
                .map_err(|_| invalid("late Source HTTP fixture did not settle"))?
                .map_err(|error| invalid(error.to_string()))??;
            assert_eq!(captured.len(), 2, "both requests must reach actual HTTP");
            let next = captured
                .get(1)
                .ok_or_else(|| invalid("next HTTP request missing"))?;
            let sent = next
                .pointer("/messages/1/content")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("late Source native replay missing"))?;
            let expected = original
                .get("content")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("original native content missing"))?;
            assert_eq!(
                sent.len(),
                expected.len(),
                "no duplicate reasoning/tool or runtime-only group"
            );
            for (sent, expected) in sent.iter().zip(expected) {
                let mut sent = sent.clone();
                if let Some(fields) = sent.as_object_mut() {
                    fields.remove("cache_control");
                }
                assert_eq!(
                    &sent, expected,
                    "late Source closure lost prefix/full payload or changed order"
                );
            }
            assert!(!next.to_string().contains("runtime-only-must-not-be-wire"));
            assert!(!next.to_string().contains("provider_source"));
            assert!(!next.to_string().contains("approved_assistant_blocks"));
        }
    }
    Ok(())
}

#[test]
fn metadata_late_source_replay_rejects_mutation_foreign_and_cross_group_splicing() -> Result<()> {
    use super::compatibility_tests::reload;
    let client = AnthropicClient::new("fixture-only", "test-model");
    let raw = json!({"id":"native-late-control","content":[{"type":"text","text":"native-full"}],"stop_reason":"end_turn"});
    let original = client
        .convert_response(serde_json::from_value(raw).map_err(|error| invalid(error.to_string()))?)?
        .message;
    let complete = original
        .assistant_blocks
        .as_ref()
        .and_then(|blocks| blocks.first())
        .cloned()
        .ok_or_else(|| invalid("native block control missing"))?;
    let mut prefix = complete.clone();
    prefix.provider_source = None;
    prefix.complete = false;
    let mut closure = complete.clone();
    closure.delta.clear();
    closure.sequence = closure.sequence.saturating_add(1);
    for fault in [
        "source_change",
        "source_removed",
        "foreign_source",
        "duplicate_position",
        "wrong_observed_id",
        "provider_change",
        "late_provider_change",
        "late_provider_missing",
    ] {
        let mut message = original.clone();
        let mut before = prefix.clone();
        let mut after = closure.clone();
        let mut middle = Vec::new();
        match fault {
            "source_change" => {
                before.provider_source = complete.provider_source.clone();
                after
                    .provider_source
                    .as_mut()
                    .ok_or_else(|| invalid("source missing"))?
                    .item_id = "different-item".into();
            }
            "source_removed" => {
                before.provider_source = complete.provider_source.clone();
                after.provider_source = None;
            }
            "foreign_source" => {
                before.provider_source = complete.provider_source.clone();
                after
                    .provider_source
                    .as_mut()
                    .ok_or_else(|| invalid("source missing"))?
                    .protocol = LlmApiProtocol::Responses;
            }
            "duplicate_position" => {
                before.provider_source = complete.provider_source.clone();
                after.block_id = "different-canonical-group".into();
            }
            "wrong_observed_id" => {
                after
                    .provider_source
                    .as_mut()
                    .ok_or_else(|| invalid("source missing"))?
                    .request_id = "different-native-owner".into()
            }
            "provider_change" => before.provider = Some("foreign-provider".into()),
            "late_provider_change" | "late_provider_missing" => {
                before.provider = None;
                let mut foreign = prefix.clone();
                foreign.provider = Some("foreign-provider".into());
                foreign.delta = "foreign-middle-prefix".into();
                foreign.sequence = foreign.sequence.saturating_add(1);
                middle.push(foreign);
                after.sequence = after.sequence.saturating_add(2);
                if fault == "late_provider_missing" {
                    after.provider = None;
                }
            }
            _ => return Err(invalid("unknown late Source test case").into()),
        }
        let mut blocks = vec![before];
        blocks.extend(middle);
        blocks.push(after);
        message.assistant_blocks = Some(blocks);
        assert!(
            client
                .convert_request(&ChatRequest::new(vec![reload(&message)?]))
                .is_err(),
            "late Source must reject {fault}"
        );
    }
    // 相同block_id但不同canonical request的无Source前缀也不能借另组的Source。
    let mut foreign = prefix;
    foreign.request_id = "foreign-canonical-request".into();
    foreign.delta = "foreign-prefix".into();
    let mut message = original;
    message.assistant_blocks = Some(vec![foreign, complete]);
    let wire =
        serde_json::to_value(client.convert_request(&ChatRequest::new(vec![reload(&message)?]))?)
            .map_err(|error| invalid(error.to_string()))?;
    assert_eq!(
        wire.pointer("/messages/0/content/0/text"),
        Some(&json!("native-full"))
    );
    assert!(!wire.to_string().contains("foreign-prefix"));
    Ok(())
}
