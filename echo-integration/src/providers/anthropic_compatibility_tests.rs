//! 真实 loopback HTTP 请求捕获；只有协议证据，不赋予 ReAct/EKO/SDK 最终完成资格。

use super::*;
use echo_core::llm::capabilities::{
    ModelFactConfidence, ModelFactMetadata, ModelFactSet, ModelProfileOverride,
};
use echo_core::llm::types::{FunctionSpec, ImageUrl, ToolDefinition};
use echo_core::llm::{ThinkingConfig, ThinkingLevel};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn error(reason: impl Into<String>) -> LlmError {
    LlmError::InvalidResponse(reason.into())
}

fn sourced_client(url: String, thinking_protocol: ThinkingProtocol) -> AnthropicClient {
    let facts = ModelFactSet::new_partial(
        ModelFactMetadata::new(
            ModelFactSource::ExactModel,
            "loopback fixture contract; not a live service observation",
            "anthropic-compatibility-v1",
            SystemTime::now(),
            None,
            ModelFactConfidence::VERIFIED,
        ),
        ProviderCapabilityOverride {
            structured_output: Some(true),
            tool_support: Some(true),
            ..Default::default()
        },
        ModelProfileOverride {
            supports_structured_output: Some(true),
            thinking_protocol: Some(thinking_protocol),
            ..Default::default()
        },
    );
    AnthropicClient::with_base_url(url, "fixture-only-not-a-secret", "fixture-model")
        .with_model_profile_resolver(
            "fixture-provider",
            ModelProfileResolver::new().register_exact_model_facts(
                "fixture-provider",
                "fixture-model",
                facts,
            ),
        )
}

pub(super) fn request(strict: Option<bool>) -> ChatRequest {
    let mut request = ChatRequest::new(vec![Message::user("原始请求😀".into())]);
    request.tools = Some(vec![ToolDefinition {
        tool_type: "function".into(),
        function: FunctionSpec {
            name: "lookup".into(),
            description: "读取资料".into(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
            strict,
        },
    }]);
    request
}

fn json_wire(client: &AnthropicClient, request: &ChatRequest) -> Result<Value> {
    serde_json::to_value(client.convert_request(request)?)
        .map_err(|err| LlmError::SerializationError(err.to_string()).into())
}

pub(super) fn fixture_rounds() -> Result<Vec<Value>> {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../fixtures/providers/anthropic/compatibility.json"
    ))
    .map_err(|err| error(err.to_string()))?;
    fixture
        .get("rounds")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| error("round fixture missing").into())
}

#[test]
fn compatibility_tool_choices_are_native_and_named_tool_must_exist() -> Result<()> {
    let client = AnthropicClient::new("fixture-only", "claude-sonnet-4-5");
    let mut request = request(None);
    for (choice, native) in [
        (ToolChoice::Auto, json!({"type":"auto"})),
        (ToolChoice::None, json!({"type":"none"})),
        (ToolChoice::Required, json!({"type":"any"})),
        (
            ToolChoice::function("lookup"),
            json!({"type":"tool","name":"lookup"}),
        ),
    ] {
        request.tool_choice = Some(choice);
        assert_eq!(
            json_wire(&client, &request)?.get("tool_choice"),
            Some(&native)
        );
    }
    request.tool_choice = Some(ToolChoice::function("missing"));
    assert!(client.convert_request(&request).is_err());
    request.tool_choice = Some(ToolChoice::function(""));
    assert!(client.convert_request(&request).is_err());
    request.tools = None;
    request.tool_choice = Some(ToolChoice::Required);
    assert!(client.convert_request(&request).is_err());
    request.tool_choice = Some(ToolChoice::None);
    assert_eq!(
        json_wire(&client, &request)?.get("tool_choice"),
        Some(&json!({"type":"none"}))
    );
    Ok(())
}

#[test]
fn compatibility_strict_function_and_schema_final_are_independent() -> Result<()> {
    let client = AnthropicClient::new("fixture-only", "claude-sonnet-4-5");
    for strict in [None, Some(false), Some(true)] {
        let mut request = request(strict);
        let native = json_wire(&client, &request)?;
        assert_eq!(
            native.pointer("/tools/0/strict").cloned(),
            strict.map(Value::Bool)
        );
        assert!(native.get("output_config").is_none());
        let schema = json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false});
        request.response_format = Some(ResponseFormat::json_schema("answer", schema.clone()));
        let native = json_wire(&client, &request)?;
        assert_eq!(
            native.pointer("/output_config/format"),
            Some(&json!({"type":"json_schema","schema":schema}))
        );
        assert!(native.get("response_format").is_none());
        assert!(native.pointer("/output_config/format/strict").is_none());
        assert!(native.pointer("/output_config/format/name").is_none());
        request.response_format = Some(ResponseFormat::Text);
        assert!(json_wire(&client, &request)?.get("output_config").is_none());
        request.response_format = Some(ResponseFormat::JsonObject);
        assert!(client.convert_request(&request).is_err());
    }
    let unknown =
        AnthropicClient::with_base_url("http://127.0.0.1/unused", "fixture-only", "unknown-model");
    assert!(unknown.convert_request(&request(Some(true))).is_err());
    let mut schema_request = request(None);
    schema_request.response_format = Some(ResponseFormat::json_schema(
        "x",
        json!({"type":"object","additionalProperties":false}),
    ));
    assert!(unknown.convert_request(&schema_request).is_err());
    // 已验证 official GA 模型的默认入口可编码；未发网络请求。
    let official = AnthropicClient::new("fixture-only", "claude-sonnet-4-5");
    assert!(official.capabilities().structured_output);
    assert!(
        json_wire(&official, &schema_request)?
            .pointer("/output_config/format")
            .is_some()
    );
    Ok(())
}

#[test]
fn compatibility_effort_is_nested_and_invalid_thinking_combinations_error() -> Result<()> {
    let adaptive = sourced_client(
        "http://127.0.0.1/unused".into(),
        ThinkingProtocol::AnthropicEffort,
    );
    let mut request = request(None);
    request.thinking = Some(ThinkingConfig::Level(ThinkingLevel::High));
    let native = json_wire(&adaptive, &request)?;
    assert_eq!(native.pointer("/thinking/type"), Some(&json!("adaptive")));
    assert_eq!(
        native.pointer("/output_config/effort"),
        Some(&json!("high"))
    );
    assert!(native.get("effort").is_none());
    request.tool_choice = Some(ToolChoice::Required);
    assert!(adaptive.convert_request(&request).is_err());
    request.tool_choice = Some(ToolChoice::function("lookup"));
    assert!(adaptive.convert_request(&request).is_err());
    request.tool_choice = Some(ToolChoice::Auto);
    request.temperature = Some(0.5);
    assert!(adaptive.convert_request(&request).is_err());
    request.thinking = Some(ThinkingConfig::Disabled);
    assert_eq!(
        json_wire(&adaptive, &request)?.pointer("/thinking/type"),
        Some(&json!("disabled"))
    );
    request.temperature = None;
    request.thinking = Some(ThinkingConfig::BudgetTokens(2048));
    assert!(adaptive.convert_request(&request).is_err());
    let budget = sourced_client(
        "http://127.0.0.1/unused".into(),
        ThinkingProtocol::AnthropicThinkingBudget,
    );
    request.max_tokens = Some(4096);
    for invalid in [0, 1023, 4096, u32::MAX] {
        request.thinking = Some(ThinkingConfig::BudgetTokens(invalid));
        assert!(budget.convert_request(&request).is_err());
    }
    request.thinking = Some(ThinkingConfig::BudgetTokens(2048));
    assert_eq!(
        json_wire(&budget, &request)?.pointer("/thinking/budget_tokens"),
        Some(&json!(2048))
    );
    let unknown = sourced_client("http://127.0.0.1/unused".into(), ThinkingProtocol::None);
    assert!(unknown.convert_request(&request).is_err());
    request.thinking = None;
    for invalid in [-0.1, 1.1, f32::NAN, f32::INFINITY] {
        request.temperature = Some(invalid);
        assert!(unknown.convert_request(&request).is_err());
    }
    // 用已证 native endpoint/model 和明确的测试方言，确保真正走到组合 guard。
    let native_facts = ModelFactSet::new_partial(
        ModelFactMetadata::new(
            ModelFactSource::ExactModel,
            "unit native thinking dialect; not live evidence",
            "thinking-combination-control-v1",
            SystemTime::now(),
            None,
            ModelFactConfidence::VERIFIED,
        ),
        ProviderCapabilityOverride::default(),
        ModelProfileOverride {
            thinking_protocol: Some(ThinkingProtocol::AnthropicThinkingBudget),
            ..Default::default()
        },
    );
    let native = AnthropicClient::new("fixture-only", "claude-sonnet-4-5")
        .with_model_profile_resolver(
            "anthropic",
            ModelProfileResolver::new().register_exact_model_facts(
                "anthropic",
                "claude-sonnet-4-5",
                native_facts,
            ),
        );
    let mut forced_request = self::request(None);
    forced_request.max_tokens = Some(4096);
    for choice in [ToolChoice::Required, ToolChoice::function("lookup")] {
        forced_request.tool_choice = Some(choice);
        forced_request.thinking = Some(ThinkingConfig::BudgetTokens(2048));
        let failure = native
            .convert_request(&forced_request)
            .err()
            .ok_or_else(|| error("active thinking and forced choice unexpectedly accepted"))?;
        assert!(
            failure.to_string().contains("thinking + tool_choice"),
            "unexpected earlier rejection: {failure}"
        );
        forced_request.thinking = Some(ThinkingConfig::Disabled);
        assert!(
            native.convert_request(&forced_request).is_ok(),
            "same forced choice must work after explicit thinking disable"
        );
    }
    forced_request.thinking = Some(ThinkingConfig::BudgetTokens(2048));
    for choice in [ToolChoice::Auto, ToolChoice::None] {
        forced_request.tool_choice = Some(choice);
        assert_eq!(
            json_wire(&native, &forced_request)?.pointer("/thinking/type"),
            Some(&json!("enabled"))
        );
    }
    forced_request.temperature = Some(0.5);
    let failure = native
        .convert_request(&forced_request)
        .err()
        .ok_or_else(|| error("native thinking sampling unexpectedly accepted"))?;
    assert!(failure.to_string().contains("thinking + temperature"));
    Ok(())
}

#[test]
fn compatibility_unknown_endpoint_choices_and_final_schema_do_not_grant_strict() -> Result<()> {
    let unknown = AnthropicClient::with_base_url(
        "http://127.0.0.1/unused",
        "fixture-only",
        "claude-sonnet-4-5",
    );
    let mut request = request(None);
    for choice in [
        ToolChoice::Auto,
        ToolChoice::None,
        ToolChoice::Required,
        ToolChoice::function("lookup"),
    ] {
        request.tool_choice = Some(choice);
        assert!(
            unknown.convert_request(&request).is_err(),
            "Claude-like names on an unknown service grant no choice capability"
        );
    }
    let exact_final = sourced_client("http://127.0.0.1/unused".into(), ThinkingProtocol::None);
    request.tool_choice = None;
    request.response_format = Some(ResponseFormat::json_schema(
        "answer",
        json!({"type":"object","additionalProperties":false}),
    ));
    assert!(exact_final.convert_request(&request).is_ok());
    if let Some(tool) = request.tools.as_mut().and_then(|tools| tools.first_mut()) {
        tool.function.strict = Some(true);
    }
    assert!(
        exact_final.convert_request(&request).is_err(),
        "final output schema fact cannot grant strict tool inputs"
    );
    if let Some(tool) = request.tools.as_mut().and_then(|tools| tools.first_mut()) {
        tool.function.strict = None;
    }
    request.response_format = None;
    request.tool_choice = Some(ToolChoice::Auto);
    assert!(exact_final.convert_request(&request).is_ok());
    request.tool_choice = Some(ToolChoice::Required);
    assert!(
        exact_final.convert_request(&request).is_err(),
        "ordinary tool support is not forced choice evidence"
    );
    Ok(())
}

#[tokio::test]
async fn compatibility_http_all_choice_and_strict_values_are_native_in_both_modes() -> Result<()> {
    for streaming in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let mut captures = Vec::new();
            for index in 0..12 {
                let (mut socket, _) = listener.accept().await?;
                let request = read_request(&mut socket).await?;
                let forced = matches!(
                    request.pointer("/tool_choice/type").and_then(Value::as_str),
                    Some("any" | "tool")
                );
                let response = if forced {
                    json!({"id":format!("control_{index}"),"content":[{"type":"tool_use","id":"control_call","name":"lookup","input":{"path":"中文"}}],"stop_reason":"tool_use","usage":{"input_tokens":2,"output_tokens":1}})
                } else {
                    json!({"id":format!("control_{index}"),"content":[{"type":"text","text":"{\"answer\":\"ok\"}"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}})
                };
                captures.push(request);
                if streaming {
                    write_response(
                        &mut socket,
                        "text/event-stream",
                        &encode_events(&sse_events(&response)?),
                    )
                    .await?;
                } else {
                    write_response(&mut socket, "application/json", &response.to_string()).await?;
                }
            }
            Ok::<_, echo_core::error::ReactError>(captures)
        });
        let mut client = AnthropicClient::with_base_url(
            format!("http://api.anthropic.com:{}/v1/messages", address.port()),
            "fixture-only",
            "claude-sonnet-4-5",
        );
        client.client = Arc::new(
            Client::builder()
                .no_proxy()
                .resolve("api.anthropic.com", address)
                .build()
                .map_err(|err| error(err.to_string()))?,
        );
        let choices = [
            (ToolChoice::Auto, json!({"type":"auto"})),
            (ToolChoice::None, json!({"type":"none"})),
            (ToolChoice::Required, json!({"type":"any"})),
            (
                ToolChoice::function("lookup"),
                json!({"type":"tool","name":"lookup"}),
            ),
        ];
        let result = async {
            for (choice, _) in &choices {
                for strict in [None, Some(false), Some(true)] {
                    let mut request = request(strict);
                    request.tool_choice = Some(choice.clone());
                    request.response_format = Some(ResponseFormat::json_schema("answer", json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false})));
                    collect_message(&client, request, streaming).await?;
                }
            }
            Ok::<_, echo_core::error::ReactError>(())
        }.await;
        if let Err(err) = result {
            server.abort();
            let _settled = server.await;
            return Err(err);
        }
        let captures = tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .map_err(|_| error("controls HTTP fixture did not settle"))?
            .map_err(|err| error(err.to_string()))??;
        for (choice_group, (_, native)) in captures.chunks(3).zip(choices.iter()) {
            for (captured, strict) in choice_group.iter().zip([None, Some(false), Some(true)]) {
                assert_eq!(captured.get("tool_choice"), Some(native));
                assert_eq!(
                    captured.pointer("/tools/0/strict").cloned(),
                    strict.map(Value::Bool)
                );
                assert_eq!(
                    captured.pointer("/output_config/format/type"),
                    Some(&json!("json_schema"))
                );
                assert!(captured.get("effort").is_none());
                assert!(captured.get("response_format").is_none());
            }
        }
    }
    Ok(())
}

pub(super) async fn read_request(socket: &mut TcpStream) -> Result<Value> {
    let mut bytes = Vec::new();
    loop {
        if socket.read_buf(&mut bytes).await? == 0 {
            return Err(error("HTTP request truncated").into());
        }
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(error("fixture request exceeds bound").into());
        }
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(err) if err.error_len().is_none() => continue,
            Err(_) => return Err(error("fixture request invalid UTF-8").into()),
        };
        if let Some((headers, body)) = text.split_once("\r\n\r\n") {
            let length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>())
                })
                .transpose()
                .map_err(|err| error(err.to_string()))?
                .ok_or_else(|| error("content length missing"))?;
            if body.len() >= length {
                return serde_json::from_str(body).map_err(|err| error(err.to_string()).into());
            }
        }
    }
}

pub(super) fn sse_events(response: &Value) -> Result<Vec<Value>> {
    let mut events = vec![
        json!({"type":"message_start","message":{"id":response.get("id"),"type":"message","role":"assistant","model":"fixture-model","content":[],"stop_reason":null,"usage":{"input_tokens":response.pointer("/usage/input_tokens"),"output_tokens":0,"cache_creation_input_tokens":response.pointer("/usage/cache_creation_input_tokens"),"cache_read_input_tokens":response.pointer("/usage/cache_read_input_tokens")}}}),
    ];
    for (index, block) in response
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| error("fixture content missing"))?
        .iter()
        .enumerate()
    {
        let kind = block
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| error("fixture block type missing"))?;
        match kind {
            "text" => {
                events.push(json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}));
                events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":block.get("text")}}));
            }
            "thinking" => {
                let mut start = json!({"type":"thinking","thinking":""});
                if block.get("signature").is_some()
                    && let Some(fields) = start.as_object_mut()
                {
                    fields.insert("signature".into(), json!(""));
                }
                events.push(
                    json!({"type":"content_block_start","index":index,"content_block":start}),
                );
                events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":block.get("thinking")}}));
                if let Some(signature) = block.get("signature").and_then(Value::as_str) {
                    events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"signature_delta","signature":signature}}));
                }
            }
            "redacted_thinking" => events
                .push(json!({"type":"content_block_start","index":index,"content_block":block})),
            "tool_use" => {
                events.push(json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":block.get("id"),"name":block.get("name"),"input":{}}}));
                events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":block.get("input").ok_or_else(|| error("fixture input missing"))?.to_string()}}));
            }
            _ => return Err(error("unsupported fixture output type").into()),
        }
        events.push(json!({"type":"content_block_stop","index":index}));
    }
    events.push(json!({"type":"message_delta","delta":{"stop_reason":response.get("stop_reason"),"stop_sequence":response.get("stop_sequence")},"usage":{"output_tokens":response.pointer("/usage/output_tokens")}}));
    events.push(json!({"type":"message_stop"}));
    Ok(events)
}

pub(super) async fn write_response(
    socket: &mut TcpStream,
    content_type: &str,
    body: &str,
) -> Result<()> {
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await?;
    // 故意按字节分片写入，含中文和 emoji；不对 str 做字节切片。
    for part in body.as_bytes().chunks(7) {
        socket.write_all(part).await?;
    }
    socket.flush().await?;
    Ok(())
}

pub(super) fn encode_events(events: &[Value]) -> String {
    events
        .iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message")
            )
        })
        .collect()
}

pub(super) async fn collect_message(
    client: &AnthropicClient,
    request: ChatRequest,
    streaming: bool,
) -> Result<(Message, Option<String>, Option<Usage>)> {
    if !streaming {
        let response = client.chat(request).await?;
        return Ok((response.message, response.finish_reason, response.usage));
    }
    let mut stream = client.chat_stream(request).await?;
    let mut message = Message::assistant(String::new());
    let mut text = String::new();
    let mut blocks = Vec::new();
    let mut reasoning = Vec::new();
    let mut tools = std::collections::BTreeMap::<u32, ToolCall>::new();
    let mut finish = None;
    let mut usage = None;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if let Some(metadata) = chunk.delta.provider_response {
            message.provider_response = Some(metadata);
        }
        text.push_str(chunk.delta.content.as_deref().unwrap_or_default());
        blocks.extend(chunk.delta.assistant_blocks.unwrap_or_default());
        reasoning.extend(chunk.delta.reasoning_blocks.unwrap_or_default());
        for delta in chunk.delta.tool_calls.unwrap_or_default() {
            let call = tools.entry(delta.index).or_insert_with(|| ToolCall {
                provider_source: delta.provider_source.clone(),
                id: String::new(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: String::new(),
                    arguments: String::new(),
                },
            });
            assert_eq!(call.provider_source, delta.provider_source);
            if let Some(id) = delta.id {
                call.id.push_str(&id);
            }
            if let Some(function) = delta.function {
                call.function
                    .name
                    .push_str(function.name.as_deref().unwrap_or_default());
                call.function
                    .arguments
                    .push_str(function.arguments.as_deref().unwrap_or_default());
            }
        }
        if let Some(reason) = chunk.finish_reason {
            assert!(finish.is_none(), "duplicate protocol terminal");
            finish = Some(reason);
        }
        if chunk.usage.is_some() {
            usage = chunk.usage;
        }
    }
    message.content = MessageContent::Text(text);
    message.assistant_blocks = Some(blocks);
    message.reasoning_blocks = (!reasoning.is_empty()).then_some(reasoning);
    message.tool_calls = (!tools.is_empty()).then(|| tools.into_values().collect());
    Ok((message, finish, usage))
}

pub(super) fn reload(message: &Message) -> Result<Message> {
    let bytes = serde_json::to_vec(message).map_err(|err| error(err.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|err| error(err.to_string()).into())
}

pub(super) fn rich_result(id: &str) -> Message {
    use base64::Engine as _;
    Message {
        role: Role::Tool,
        tool_call_id: Some(id.into()),
        tool_result_is_error: Some(true),
        content: MessageContent::Parts(vec![
            ContentPart::Text {
                text: "工具失败但保留图片和文件😀".into(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,aW1hZ2U=".into(),
                    detail: None,
                },
            },
            ContentPart::File {
                name: "report.pdf".into(),
                content: base64::engine::general_purpose::STANDARD.encode(b"%PDF fixture"),
            },
            ContentPart::File {
                name: "notes.txt".into(),
                content: base64::engine::general_purpose::STANDARD.encode("完整中文😀".as_bytes()),
            },
        ]),
        ..Default::default()
    }
}

#[tokio::test]
async fn compatibility_http_three_turns_preserve_rich_error_signed_history_and_order() -> Result<()>
{
    for streaming in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let rounds = fixture_rounds()?;
        let server_rounds = rounds.clone();
        let server = tokio::spawn(async move {
            let mut captures = Vec::new();
            for response in server_rounds {
                let (mut socket, _) = listener.accept().await?;
                let captured = read_request(&mut socket).await?;
                assert_eq!(
                    captured
                        .get("stream")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    streaming
                );
                captures.push(captured);
                if streaming {
                    write_response(
                        &mut socket,
                        "text/event-stream",
                        &encode_events(&sse_events(&response)?),
                    )
                    .await?;
                } else {
                    write_response(&mut socket, "application/json", &response.to_string()).await?;
                }
            }
            Ok::<_, echo_core::error::ReactError>(captures)
        });
        let mut client = AnthropicClient::with_base_url(
            format!("http://api.anthropic.com:{}/v1/messages", address.port()),
            "fixture-only",
            "claude-sonnet-4-5",
        );
        client.client = Arc::new(
            Client::builder()
                .no_proxy()
                .resolve("api.anthropic.com", address)
                .build()
                .map_err(|err| error(err.to_string()))?,
        );
        let result = async {
            let mut request = request(Some(true));
            request.tool_choice = Some(ToolChoice::Auto);
            request.response_format = Some(ResponseFormat::json_schema("out", json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false})));
            let (first, stop, usage) = collect_message(&client, request.clone(), streaming).await?;
            assert_eq!(stop.as_deref(), Some("tool_calls"));
            assert_eq!(usage.as_ref().and_then(|value| value.cache_creation_input_tokens), Some(2));
            assert_eq!(usage.as_ref().and_then(|value| value.cache_read_input_tokens), Some(3));
            assert_eq!(usage.as_ref().and_then(|value| value.total_tokens), Some(18));
            request.messages.push(reload(&first)?);
            request.messages.push(reload(&rich_result("call_a"))?);
            request.messages.push(Message { role: Role::Tool, tool_call_id: Some("call_b".into()), content: MessageContent::Text(String::new()), tool_result_is_error: Some(false), ..Default::default() });
            let (second, stop, _) = collect_message(&client, request.clone(), streaming).await?;
            assert_eq!(stop.as_deref(), Some("tool_calls"));
            request.messages.push(reload(&second)?);
            request.messages.push(Message { role: Role::Tool, tool_call_id: Some("call_c".into()), content: MessageContent::Text("third result".into()), ..Default::default() });
            let (final_message, stop, _) = collect_message(&client, request, streaming).await?;
            assert_eq!(stop.as_deref(), Some("stop"));
            assert_eq!(final_message.content.as_text().as_deref(), Some("完成😀"));
            Ok::<_, echo_core::error::ReactError>(())
        }.await;
        if let Err(err) = result {
            server.abort();
            let _settled = server.await;
            return Err(err);
        }
        let captures = tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .map_err(|_| error("fixture server did not settle"))?
            .map_err(|err| error(err.to_string()))??;
        let second = captures
            .get(1)
            .ok_or_else(|| error("second HTTP request missing"))?;
        let native = second
            .pointer("/messages/1/content")
            .and_then(Value::as_array)
            .ok_or_else(|| error("signed replay missing"))?;
        let expected = rounds
            .first()
            .and_then(|round| round.get("content"))
            .and_then(Value::as_array)
            .ok_or_else(|| error("fixture blocks missing"))?;
        assert_eq!(native.len(), expected.len());
        for (actual, expected) in native.iter().zip(expected) {
            let mut actual = actual.clone();
            if let Some(object) = actual.as_object_mut() {
                object.remove("cache_control");
            }
            assert_eq!(&actual, expected, "native block changed during cold replay");
        }
        assert_eq!(
            second.pointer("/messages/2/content/0/is_error"),
            Some(&json!(true))
        );
        assert_eq!(
            second.pointer("/messages/2/content/1/is_error"),
            Some(&json!(false))
        );
        assert_eq!(
            second.pointer("/messages/2/content/0/content/1/source/data"),
            Some(&json!("aW1hZ2U="))
        );
        assert_eq!(
            second.pointer("/messages/2/content/0/content/2/type"),
            Some(&json!("document"))
        );
        assert!(
            second
                .pointer("/messages/2/content/0/content/3/text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("完整中文😀"))
        );
        let third = captures
            .get(2)
            .ok_or_else(|| error("third HTTP request missing"))?;
        assert!(third.pointer("/messages/4/content/0/is_error").is_none());
        assert_eq!(
            third.pointer("/messages/3/content/1/id"),
            Some(&json!("call_c"))
        );
        assert_eq!(third.pointer("/tools/0/strict"), Some(&json!(true)));
        assert_eq!(
            third.pointer("/output_config/format/type"),
            Some(&json!("json_schema"))
        );
        assert!(!third.to_string().contains("provider_source"));
        assert!(!third.to_string().contains("tool_result_is_error"));
    }
    Ok(())
}

#[test]
fn compatibility_rich_results_reject_unsupported_binary_and_orphan_identity() -> Result<()> {
    let client = sourced_client("http://127.0.0.1/unused".into(), ThinkingProtocol::None);
    let mut request = request(None);
    request.messages.push(rich_result("missing"));
    assert!(client.convert_request(&request).is_err());
    let raw = fixture_rounds()?
        .into_iter()
        .next()
        .ok_or_else(|| error("fixture missing"))?;
    let response = client
        .convert_response(serde_json::from_value(raw).map_err(|err| error(err.to_string()))?)?;
    request.messages.pop();
    request.messages.push(response.message);
    let mut result = rich_result("call_a");
    result.content = MessageContent::Parts(vec![ContentPart::File {
        name: "archive.zip".into(),
        content: "AAE=".into(),
    }]);
    request.messages.push(result);
    assert!(client.convert_request(&request).is_err());
    Ok(())
}

#[test]
fn compatibility_pause_truncation_empty_signature_and_error_facts() -> Result<()> {
    let client = sourced_client("http://127.0.0.1/unused".into(), ThinkingProtocol::None);
    for stop in ["max_tokens", "model_context_window_exceeded", "unknown"] {
        let raw =
            json!({"id":"cut","content":[{"type":"text","text":"partial"}],"stop_reason":stop});
        assert!(
            client
                .convert_response(
                    serde_json::from_value(raw).map_err(|err| error(err.to_string()))?
                )
                .is_err()
        );
    }
    for content in [
        json!([]),
        json!([{"type":"thinking","thinking":"hidden","signature":"sig"}]),
    ] {
        let raw = json!({"id":"empty","content":content,"stop_reason":"end_turn"});
        assert!(
            client
                .convert_response(
                    serde_json::from_value(raw).map_err(|err| error(err.to_string()))?
                )
                .is_err()
        );
    }
    let pause = json!({"id":"pause","content":[{"type":"thinking","thinking":"continue","signature":"sig"}],"stop_reason":"pause_turn","stop_sequence":null,"model":"fixture-model","usage":{"input_tokens":4,"output_tokens":2}});
    let response = client.convert_response(
        serde_json::from_value(pause.clone()).map_err(|err| error(err.to_string()))?,
    )?;
    assert_eq!(response.finish_reason.as_deref(), Some("continue"));
    assert_eq!(
        response
            .raw
            .extra
            .as_ref()
            .and_then(|raw| raw.get("stop_reason")),
        Some(&json!("pause_turn"))
    );
    let mut adapter = NativeAnthropicStream::default();
    let mut chunks = Vec::new();
    for event in sse_events(&pause)? {
        chunks.extend(adapter.map(event)?);
    }
    assert_eq!(
        chunks
            .last()
            .and_then(|chunk| chunk.finish_reason.as_deref()),
        Some("continue")
    );
    let mut adapter = NativeAnthropicStream::default();
    let failure = adapter.map(json!({"type":"error","error":{"type":"overloaded_error","message":"PRIVATE_BODY_MUST_NOT_LEAK"}})).err().ok_or_else(|| error("provider error was accepted"))?;
    assert!(
        matches!(failure, echo_core::error::ReactError::Llm(ref inner)
        if matches!(inner.as_ref(), LlmError::ProviderError { provider, error_type, code: None, message, retryable: false }
            if provider.is_empty() && error_type.as_deref() == Some("overloaded_error") && message == "PRIVATE_BODY_MUST_NOT_LEAK"))
    );
    let projected = echo_core::error::AgentFailure::try_from(&failure)?;
    assert_eq!(projected.code, "llm_provider");
    assert_eq!(projected.http_status, None);
    assert!(!projected.retryable);
    Ok(())
}

#[test]
fn compatibility_deepseek_alias_does_not_inherit_claude_native_features() -> Result<()> {
    let client = AnthropicClient::with_base_url(
        "https://api.deepseek.com/anthropic/v1/messages",
        "fixture-only",
        "claude-opus-4-5",
    );
    assert!(!client.capabilities().structured_output);
    let mut request = request(None);
    request.thinking = Some(ThinkingConfig::Disabled);
    for (choice, native) in [
        (ToolChoice::None, json!({"type":"none"})),
        (ToolChoice::Auto, json!({"type":"auto"})),
        (ToolChoice::Required, json!({"type":"any"})),
        (
            ToolChoice::function("lookup"),
            json!({"type":"tool","name":"lookup"}),
        ),
    ] {
        request.tool_choice = Some(choice);
        assert_eq!(
            json_wire(&client, &request)?.get("tool_choice"),
            Some(&native)
        );
    }
    request.tool_choice = Some(ToolChoice::Auto);
    request.temperature = Some(2.0);
    request.thinking = Some(ThinkingConfig::Level(ThinkingLevel::High));
    let native = json_wire(&client, &request)?;
    assert_eq!(native.pointer("/thinking/type"), Some(&json!("enabled")));
    assert_eq!(
        native.pointer("/output_config/effort"),
        Some(&json!("high"))
    );
    assert_eq!(native.get("temperature"), Some(&json!(2.0)));
    request.thinking = Some(ThinkingConfig::BudgetTokens(2048));
    assert!(client.convert_request(&request).is_err());
    request.thinking = Some(ThinkingConfig::Disabled);
    request.response_format = Some(ResponseFormat::json_schema("x", json!({"type":"object"})));
    assert!(client.convert_request(&request).is_err());
    request.response_format = None;
    if let Some(tool) = request.tools.as_mut().and_then(|tools| tools.first_mut()) {
        tool.function.strict = Some(true);
    }
    assert!(client.convert_request(&request).is_err());
    Ok(())
}

#[tokio::test]
async fn compatibility_http_deepseek_unsigned_thinking_is_replayed_without_fabricated_signature()
-> Result<()> {
    for (streaming, signature) in [
        (false, None),
        (true, None),
        (false, Some("")),
        (true, Some("")),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let mut full = json!({"id":"deepseek_first","content":[{"type":"thinking","thinking":"未签名原文😀"},{"type":"tool_use","id":"deepseek_call","name":"lookup","input":{"path":"中文"}}],"stop_reason":"tool_use","usage":{"input_tokens":3,"output_tokens":4}});
        if let Some(signature) = signature
            && let Some(fields) = full
                .pointer_mut("/content/0")
                .and_then(Value::as_object_mut)
        {
            fields.insert("signature".into(), json!(signature));
        }
        let final_body = json!({"id":"deepseek_second","content":[{"type":"text","text":"done"}],"stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":2}});
        let server = tokio::spawn(async move {
            let mut captures = Vec::new();
            for response in [full, final_body] {
                let (mut socket, _) = listener.accept().await?;
                captures.push(read_request(&mut socket).await?);
                if streaming {
                    write_response(
                        &mut socket,
                        "text/event-stream",
                        &encode_events(&sse_events(&response)?),
                    )
                    .await?;
                } else {
                    write_response(&mut socket, "application/json", &response.to_string()).await?;
                }
            }
            Ok::<_, echo_core::error::ReactError>(captures)
        });
        // 保留真实 vendor URL 分类，仅 HTTP DNS 定向 loopback；不会访问 DeepSeek 服务。
        let mut client = AnthropicClient::with_base_url(
            format!(
                "http://api.deepseek.com:{}/anthropic/v1/messages",
                address.port()
            ),
            "fixture-only",
            "claude-opus-4-5",
        );
        client.client = Arc::new(
            Client::builder()
                .no_proxy()
                .resolve("api.deepseek.com", address)
                .build()
                .map_err(|err| error(err.to_string()))?,
        );
        let result = async {
            let mut request = request(None);
            let (first, stop, _) = collect_message(&client, request.clone(), streaming).await?;
            assert_eq!(stop.as_deref(), Some("tool_calls"));
            assert!(first.reasoning_blocks.as_ref().is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| matches!(block, ReasoningBlock::Opaque { .. }))
            }));
            request.messages.push(reload(&first)?);
            request.messages.push(Message {
                role: Role::Tool,
                tool_call_id: Some("deepseek_call".into()),
                content: MessageContent::Text("result".into()),
                ..Default::default()
            });
            let (last, stop, _) = collect_message(&client, request, streaming).await?;
            assert_eq!(last.content.as_text().as_deref(), Some("done"));
            assert_eq!(stop.as_deref(), Some("stop"));
            Ok::<_, echo_core::error::ReactError>(())
        }
        .await;
        if let Err(err) = result {
            server.abort();
            let _settled = server.await;
            return Err(err);
        }
        let captures = tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .map_err(|_| error("DeepSeek loopback did not settle"))?
            .map_err(|err| error(err.to_string()))??;
        let replay = captures
            .get(1)
            .ok_or_else(|| error("DeepSeek replay request missing"))?;
        assert_eq!(
            replay.pointer("/messages/1/content/0/thinking"),
            Some(&json!("未签名原文😀"))
        );
        assert_eq!(
            replay.pointer("/messages/1/content/0/signature").cloned(),
            signature.map(|value| json!(value))
        );
        assert!(replay.pointer("/messages/2/content/0/is_error").is_none());
    }
    Ok(())
}

#[tokio::test]
async fn compatibility_http_abnormal_eof_never_emits_success() -> Result<()> {
    for body in [
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"cut\"}}\n\n"
            .to_string(),
        "data: {\"type\":\"message_start\"}".to_string(),
        String::new(),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let request_received = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let server_received = request_received.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            read_request(&mut socket).await?;
            server_received.store(true, std::sync::atomic::Ordering::SeqCst);
            write_response(&mut socket, "text/event-stream", &body).await
        });
        let client = sourced_client(format!("http://{address}"), ThinkingProtocol::None);
        let mut failed = false;
        let mut has_finish = false;
        let mut creation_failure = None;
        // 空HTTP/SSE可在首字节/首事件接纳时失败；两种合法失败时点均必须覆盖。
        match client.chat_stream(request(None)).await {
            Err(failure) => {
                failed = true;
                creation_failure = Some(failure);
            }
            Ok(mut stream) => {
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(chunk) => has_finish |= chunk.finish_reason.is_some(),
                        Err(_) => failed = true,
                    }
                }
            }
        }
        if !request_received.load(std::sync::atomic::Ordering::SeqCst) {
            // 前置能力/请求失败不能冒EOF反控通过；收口未接到请求的原HTTP夹具。
            server.abort();
            let _settled = server.await;
            return Err(creation_failure.unwrap_or_else(|| {
                error("EOF fixture did not receive the production HTTP request").into()
            }));
        }
        server.await.map_err(|err| error(err.to_string()))??;
        assert!(failed, "EOF must fail during stream creation or iteration");
        assert!(!has_finish, "EOF cannot publish a successful finish");
    }
    Ok(())
}

#[tokio::test]
async fn compatibility_http_cancel_during_body_settles_both_modes() -> Result<()> {
    for streaming in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let ready = std::sync::Arc::new(tokio::sync::Notify::new());
        let server_ready = ready.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            read_request(&mut socket).await?;
            let prefix = if streaming {
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"cancel\"}}\n\n"
            } else {
                "{\"id\":\"cancel\","
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: 99999\r\nConnection: close\r\n\r\n{prefix}", if streaming {"text/event-stream"} else {"application/json"}).as_bytes()).await?;
            socket.flush().await?;
            server_ready.notify_one();
            let mut byte = [0_u8; 1];
            match socket.read(&mut byte).await {
                Ok(0) => {}
                Err(io_error)
                    if matches!(
                        io_error.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                    ) => {}
                Ok(_) => {
                    return Err(error(
                        "client cancellation must release HTTP body without extra bytes",
                    )
                    .into());
                }
                Err(io_error) => return Err(io_error.into()),
            }
            Ok::<_, echo_core::error::ReactError>(())
        });
        let client = std::sync::Arc::new(sourced_client(
            format!("http://{address}"),
            ThinkingProtocol::None,
        ));
        let token = tokio_util::sync::CancellationToken::new();
        let mut request = request(None);
        request.cancel_token = Some(token.clone());
        let operation = tokio::spawn(async move {
            if streaming {
                let mut stream = client.chat_stream(request).await?;
                while let Some(chunk) = stream.next().await {
                    assert!(chunk?.finish_reason.is_none());
                }
                Ok::<_, echo_core::error::ReactError>(())
            } else {
                client.chat(request).await.map(|_| ())
            }
        });
        tokio::time::timeout(Duration::from_secs(3), ready.notified())
            .await
            .map_err(|_| error("cancellation fixture was not accepted"))?;
        token.cancel();
        let result = tokio::time::timeout(Duration::from_secs(3), operation)
            .await
            .map_err(|_| error("cancelled HTTP did not settle"))?
            .map_err(|err| error(err.to_string()))?;
        assert!(result.is_err());
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .map_err(|_| error("fixture socket did not close"))?
            .map_err(|err| error(err.to_string()))??;
    }
    Ok(())
}
