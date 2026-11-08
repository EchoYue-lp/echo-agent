//! 真实Root typed history/File cold→configured AnthropicClient→原HTTP；只追加测试。
use crate::agent::{Agent, AgentEvent, ReactAgentBuilder};
use crate::error::{ReactError, Result};
use crate::llm::{ChatChunk, ChatRequest, ChatResponse, LlmApiProtocol, LlmClient, LlmConfig};
use crate::memory::ConversationStore;
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

struct RealClientRecorder {
    inner: Arc<dyn LlmClient>,
    requests: Mutex<Vec<ChatRequest>>,
}
impl RealClientRecorder {
    fn keep(&self, request: &ChatRequest) -> Result<()> {
        self.requests
            .lock()
            .map_err(|error| ReactError::Other(error.to_string()))?
            .push(request.clone());
        Ok(())
    }
}
impl LlmClient for RealClientRecorder {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        Box::pin(async move {
            self.keep(&request)?;
            self.inner.chat(request).await
        })
    }
    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>> {
        Box::pin(async move {
            self.keep(&request)?;
            self.inner.chat_stream(request).await
        })
    }
    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
    fn provider_name(&self) -> Option<&str> {
        self.inner.provider_name()
    }
    fn capabilities(&self) -> crate::llm::ProviderCapabilities {
        self.inner.capabilities()
    }
    fn protocol_capabilities(&self) -> crate::llm::ProviderCapabilityOverride {
        self.inner.protocol_capabilities()
    }
    fn model_profile_resolution(&self) -> crate::llm::ModelProfileResolution {
        self.inner.model_profile_resolution()
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> std::io::Result<(String, Value)> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let count = socket.read(&mut chunk).await?;
        if count == 0 || bytes.len() > 2_000_000 {
            return Err(std::io::Error::other("incomplete request"));
        }
        bytes.extend_from_slice(
            chunk
                .get(..count)
                .ok_or_else(|| std::io::Error::other("bad read count"))?,
        );
        let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") else {
            continue;
        };
        let header = String::from_utf8_lossy(
            bytes
                .get(..end)
                .ok_or_else(|| std::io::Error::other("missing header"))?,
        );
        let length = header
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .ok_or_else(|| std::io::Error::other("missing content length"))?;
        let start = end
            .checked_add(4)
            .ok_or_else(|| std::io::Error::other("offset overflow"))?;
        let stop = start
            .checked_add(length)
            .ok_or_else(|| std::io::Error::other("body overflow"))?;
        if let Some(body) = bytes.get(start..stop) {
            return Ok((
                header.lines().next().unwrap_or_default().to_string(),
                serde_json::from_slice(body).map_err(std::io::Error::other)?,
            ));
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum HistoryCase {
    MissingObservedId,
    MismatchedId,
    UnknownSource,
    Known,
}

fn err(error: impl ToString) -> ReactError {
    ReactError::Other(error.to_string())
}

fn historical(case: HistoryCase) -> Result<Vec<crate::llm::types::Message>> {
    use crate::llm::types::Message;
    let (observed, source) = match case {
        HistoryCase::MissingObservedId => (None, "history-native"),
        HistoryCase::MismatchedId => (Some("observed-original"), "other-native"),
        HistoryCase::UnknownSource => (Some("history-native"), ""),
        HistoryCase::Known => (Some("history-native"), "history-native"),
    };
    let mut metadata = json!({"protocol":"anthropic","provider":"anthropic","usage":{"source":"durable-original"}});
    if let Some(id) = observed {
        metadata
            .as_object_mut()
            .ok_or_else(|| err("metadata must be object"))?
            .insert("request_id".into(), json!(id));
    }
    let source_at = |index: u32, kind: &str, item: &str| {
        json!({"protocol":"anthropic","request_id":source,
        "item_id":item,"output_index":index,"content_kind":kind})
    };
    let block = |index: u32, kind: &str, phase: &str, text: &str, reasoning: Option<Value>| {
        let mut block = json!({"block_id":format!("history-block-{index}"),"request_id":"canonical-history-owner",
            "sequence":index,"phase":phase,"delta":text,"complete":true,"provider":"anthropic",
            "provider_source":source_at(index,kind,&format!("history-native:content:{index}"))});
        if let Some(reasoning) = reasoning
            && let Some(object) = block.as_object_mut()
        {
            object.insert("reasoning".into(), reasoning);
        }
        block
    };
    let known = matches!(case, HistoryCase::Known);
    let blocks = if known {
        vec![
            block(
                0,
                "thinking",
                "reasoning",
                "",
                Some(
                    json!({"type":"signed","thinking":"signed-original","signature":"signature-original=="}),
                ),
            ),
            block(
                1,
                "redacted_thinking",
                "reasoning",
                "",
                Some(json!({"type":"redacted","data":"redacted-original=="})),
            ),
            block(2, "text", "commentary", "historical-source-body", None),
        ]
    } else {
        vec![block(
            0,
            "text",
            "commentary",
            "historical-source-body",
            None,
        )]
    };
    let mut message = json!({"role":"assistant","content":"historical-source-body","assistant_blocks":blocks,"provider_response":metadata});
    if known {
        message.as_object_mut().ok_or_else(|| err("history message must be object"))?.insert("tool_calls".into(),json!([{
            "id":"history-call","type":"function","function":{"name":"history_tool","arguments":"{\"original\":true}"},
            "provider_source":source_at(3,"tool_use","history-call")
        }]));
    }
    let message: Message = serde_json::from_value(message).map_err(err)?;
    let only = Message::provider_response_observation(
        serde_json::from_value(json!({
            "protocol":"anthropic","provider":"metadata-only-marker","request_id":"observation-only"
        }))
        .map_err(err)?,
    );
    let mut history = vec![
        Message::user("original historical user".into()),
        only,
        message,
    ];
    if known {
        history.push(Message::tool_result(
            "history-call".into(),
            "history_tool".into(),
            "historical-tool-body".into(),
        ));
    }
    Ok(history)
}

fn response_body(streaming: bool, ordinal: usize) -> String {
    let id = format!("native-reply-{ordinal}");
    if !streaming {
        return json!({"id":id,"type":"message","role":"assistant","model":"claude-sonnet-4-5",
            "content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}).to_string();
    }
    let event = |name: &str, data: Value| format!("event: {name}\ndata: {data}\n\n");
    [event("message_start",json!({"type":"message_start","message":{"id":id,"type":"message","role":"assistant",
        "model":"claude-sonnet-4-5","content":[],"usage":{"input_tokens":1,"output_tokens":0}}})),
     event("content_block_start",json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"OK"}})),
     event("content_block_stop",json!({"type":"content_block_stop","index":0})),
     event("message_delta",json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}})),
     event("message_stop",json!({"type":"message_stop"}))].concat()
}

struct ServerAbort(tokio::task::JoinHandle<Result<()>>);
impl Drop for ServerAbort {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn contains_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key(key) || map.values().any(|value| contains_key(value, key))
        }
        Value::Array(values) => values.iter().any(|value| contains_key(value, key)),
        _ => false,
    }
}

async fn run_case(case: HistoryCase) -> Result<()> {
    for streaming in [false, true] {
        let runtime_root = super::consumer_test_artifacts::root()?;
        let directory = tempfile::tempdir_in(runtime_root).map_err(err)?.keep();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(err)?;
        let endpoint = format!("http://{}/v1/messages", listener.local_addr().map_err(err)?);
        let wire_requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let accepted = Arc::new(AtomicUsize::new(0));
        let server_wire = wire_requests.clone();
        let server_accepted = accepted.clone();
        let stop = CancellationToken::new();
        let server_stop = stop.clone();
        let mut server = ServerAbort(tokio::spawn(async move {
            loop {
                let (mut socket, _) = tokio::select! {
                    _ = server_stop.cancelled() => return Ok(()),
                    accepted = listener.accept() => accepted.map_err(err)?,
                };
                server_accepted.fetch_add(1, Ordering::SeqCst);
                let (line, body) = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    read_request(&mut socket),
                )
                .await
                .map_err(err)?
                .map_err(err)?;
                if !line.starts_with("POST /v1/messages ") {
                    return Err(err(format!("unexpected fixture route: {line}")));
                }
                let count = {
                    let mut wire = server_wire.lock().map_err(err)?;
                    wire.push(body.clone());
                    wire.len()
                };
                let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
                let body = response_body(stream, count);
                let content_type = if stream {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                // HTTP Content-Length按UTF-8字节数计算，不是文本截断。
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.map_err(err)?;
                socket.shutdown().await.map_err(err)?;
            }
        }));
        let config = LlmConfig::for_provider(
            "anthropic",
            endpoint,
            "fixture-only-not-live",
            "claude-sonnet-4-5",
            LlmApiProtocol::Anthropic,
        )?;
        let client = Arc::new(RealClientRecorder {
            inner: Arc::from(config.build_client()?),
            requests: Mutex::new(Vec::new()),
        });
        let expected_history = historical(case)?;
        let expected = expected_history
            .iter()
            .find(|message| message.content.as_text().as_deref() == Some("historical-source-body"))
            .and_then(|message| message.provider_response.clone())
            .ok_or_else(|| err("fixture ordinary D1 missing"))?;
        let mut rounds = Vec::<Value>::new();
        for cold in [false, true] {
            let conversations = Arc::new(crate::memory::FileConversationStore::new(
                directory.as_path().join("conversations"),
            )?);
            let runtime = Arc::new(crate::state::FileRuntimeStateStore::new(
                directory.as_path().join("runtime"),
            )?);
            if cold {
                let restored = crate::memory::restore_messages(
                    &conversations
                        .get_messages("root-history-provenance")
                        .await?,
                )?;
                let persisted = restored
                    .iter()
                    .find(|message| {
                        message.content.as_text().as_deref() == Some("historical-source-body")
                    })
                    .ok_or_else(|| err("File cold omitted original typed history"))?;
                if persisted.provider_response.as_ref() != Some(&expected) {
                    return Err(err("File cold D1 changed before Root replay"));
                }
            }
            let mut agent = ReactAgentBuilder::new()
                .configured_llm_client(client.clone(), config.clone())
                .model_streaming(streaming)
                .auto_project_rules(false)
                .build()?;
            agent.set_conversation_id("root-history-provenance");
            agent.set_conversation_store(conversations.clone());
            agent.set_state_store(runtime.clone());
            let invocation = echo_core::agent::AgentInvocationContext {
                history: (!cold).then(|| expected_history.clone()),
                ..Default::default()
            };
            let events = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                match agent
                    .chat_stream_message_with_invocation_context(
                        crate::llm::types::Message::user("next actual Root request".into()),
                        CancellationToken::new(),
                        invocation,
                    )
                    .await
                {
                    Ok(stream) => stream.collect::<Vec<_>>().await,
                    Err(error) => vec![Err(error)],
                }
            })
            .await
            .map_err(err)?;
            let errors: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    Err(error) => Some(error.to_string()),
                    Ok(AgentEvent::Error { message, .. }) => Some(message.clone()),
                    _ => None,
                })
                .collect();
            let finals: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    Ok(AgentEvent::FinalAnswer(value)) => Some(value.clone()),
                    _ => None,
                })
                .collect();
            // 原公开owner保存原Context；不为测试清Ctx、改历史或伪造成功。
            agent.force_checkpoint().await?;
            agent.close().await?;
            drop(agent);
            drop(conversations);
            drop(runtime);
            rounds.push(json!({"cold":cold,"errors":errors,"finals":finals}));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        stop.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), &mut server.0)
            .await
            .map_err(err)?
            .map_err(err)??;
        let wire = wire_requests.lock().map_err(err)?.clone();
        let requests = client.requests.lock().map_err(err)?.clone();
        let canonical:Vec<_> = requests.iter().map(|request| {
            let ordinary = request.messages.iter().find(|message|message.content.as_text().as_deref()==Some("historical-source-body"));
            json!({"ordinary_D1":ordinary.and_then(|message|message.provider_response.as_ref()),
                "metadata_only_count":request.messages.iter().filter(|message|message.is_provider_response_observation_only()).count()})
        }).collect();
        let report = json!({"case":format!("{case:?}"),"streaming":streaming,"rounds":rounds,"actual_client_calls":requests.len(),
            "accepted_connections":accepted.load(Ordering::SeqCst),"actual_HTTP":wire.len(),"canonical":canonical,"wire":wire});
        std::fs::write(
            directory.as_path().join("result.json"),
            serde_json::to_vec_pretty(&report).map_err(err)?,
        )
        .map_err(err)?;
        eprintln!("ROOT_HISTORY_PROVENANCE {report}");
        assert_eq!(
            requests.len(),
            2,
            "must reach original client through warm invocation and new cold Agent: {report}"
        );
        if matches!(case, HistoryCase::Known) {
            assert_eq!(
                wire.len(),
                2,
                "known Source must actually send one HTTP per round: {report}"
            );
            for round in &rounds {
                assert_eq!(
                    round.get("finals"),
                    Some(&json!(["OK"])),
                    "real Root did not finish: {report}"
                );
            }
            for body in &wire {
                assert!(
                    !contains_key(body, "provider_response"),
                    "D1 must be validated then excluded from vendor wire: {report}"
                );
                let messages = body
                    .get("messages")
                    .and_then(Value::as_array)
                    .ok_or_else(|| err("actual vendor messages missing"))?;
                let blocks = messages
                    .iter()
                    .filter_map(|message| message.get("content").and_then(Value::as_array))
                    .find(|blocks| {
                        blocks.iter().any(|block| {
                            block.get("text") == Some(&json!("historical-source-body"))
                        })
                    })
                    .ok_or_else(|| err("native historical body lost"))?;
                assert_eq!(
                    blocks
                        .iter()
                        .filter_map(|block| block.get("type").and_then(Value::as_str))
                        .collect::<Vec<_>>(),
                    vec!["thinking", "redacted_thinking", "text", "tool_use"],
                    "native order changed or duplicated: {report}"
                );
                assert_eq!(
                    blocks.first().and_then(|block| block.get("signature")),
                    Some(&json!("signature-original=="))
                );
                assert_eq!(
                    blocks.get(1).and_then(|block| block.get("data")),
                    Some(&json!("redacted-original=="))
                );
                assert_eq!(
                    messages
                        .iter()
                        .filter_map(|message| message.get("content").and_then(Value::as_array))
                        .flatten()
                        .filter(|block| block.get("tool_use_id") == Some(&json!("history-call")))
                        .count(),
                    1,
                    "tool result duplicated: {report}"
                );
            }
        } else {
            assert_eq!(
                wire.len(),
                0,
                "Root must retain D1 so actual adapter refuses conflicting/unknown provenance before HTTP: {report}"
            );
            assert_eq!(
                accepted.load(Ordering::SeqCst),
                0,
                "refusal must precede connecting: {report}"
            );
            for round in &rounds {
                assert_eq!(
                    round.get("finals"),
                    Some(&json!([])),
                    "rejected history cannot publish success: {report}"
                );
                assert!(
                    round
                        .get("errors")
                        .and_then(Value::as_array)
                        .is_some_and(|errors| errors.iter().any(|value| value
                            .as_str()
                            .is_some_and(|text| text.contains("Anthropic native replay")))),
                    "must be native replay refusal, not hydration/setup failure: {report}"
                );
            }
        }
        for request in &requests {
            let ordinary = request
                .messages
                .iter()
                .find(|message| {
                    message.content.as_text().as_deref() == Some("historical-source-body")
                })
                .ok_or_else(|| err("Root silently dropped ordinary history"))?;
            assert_eq!(
                ordinary.provider_response.as_ref(),
                Some(&expected),
                "canonical D1 must reach actual adapter: {report}"
            );
            assert!(
                request
                    .messages
                    .iter()
                    .all(|message| !message.is_provider_response_observation_only()),
                "whole metadata-only record must remain filtered: {report}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn root_anthropic_history_unknown_observed_id_rejects_before_http() -> Result<()> {
    run_case(HistoryCase::MissingObservedId).await
}
#[tokio::test]
async fn root_anthropic_history_mismatched_d1_source_rejects_before_http() -> Result<()> {
    run_case(HistoryCase::MismatchedId).await
}
#[tokio::test]
async fn root_anthropic_history_unknown_source_rejects_before_http() -> Result<()> {
    run_case(HistoryCase::UnknownSource).await
}
#[tokio::test]
async fn root_anthropic_history_signed_redacted_tool_replays_after_file_cold() -> Result<()> {
    run_case(HistoryCase::Known).await
}

/// 原四项不变；只追加DeepSeek opaque的真实Root/冷恢复/官方URL代理正控。
#[tokio::test]
async fn root_deepseek_opaque_history_keeps_signature_presence_and_file_cold_order() -> Result<()> {
    const NAME: &str = "root_deepseek_opaque_history_keeps_signature_presence_and_file_cold_order";
    if let Ok(mode) = std::env::var("EKO_ROOT_DS_OPAQUE_CHILD_MODE") {
        let streaming = mode == "stream";
        let signature_present =
            std::env::var("EKO_ROOT_DS_OPAQUE_SIGNATURE_PRESENT").map_err(err)? == "yes";
        let endpoint = std::env::var("EKO_ROOT_DS_OPAQUE_ENDPOINT").map_err(err)?;
        let root = super::consumer_test_artifacts::root()?;
        let directory = tempfile::tempdir_in(root).map_err(err)?.keep();
        let config = LlmConfig::for_provider(
            "deepseek",
            endpoint,
            "fixture-only-not-live",
            "deepseek-flash",
            LlmApiProtocol::Anthropic,
        )?;
        let client = Arc::new(RealClientRecorder {
            inner: Arc::from(config.build_client()?),
            requests: Mutex::new(Vec::new()),
        });
        assert_eq!(
            client.provider_name(),
            Some("deepseek"),
            "original configured provider identity must survive proxy routing"
        );
        let mut opaque_native = json!({"type":"thinking","thinking":"opaque-original"});
        if signature_present {
            opaque_native
                .as_object_mut()
                .ok_or_else(|| err("native opaque must be object"))?
                .insert("signature".into(), json!(""));
        }
        let reasoning = json!({"type":"opaque","provider":"deepseek","id":"original-opaque-item",
            "data":opaque_native.to_string(),"summary":["original-public-summary"]});
        let source = |index: u32, kind: &str, item: &str| {
            json!({"protocol":"anthropic","request_id":"deepseek-history-native",
            "item_id":item,"output_index":index,"content_kind":kind})
        };
        let metadata = json!({"protocol":"anthropic","provider":"deepseek","request_id":"deepseek-history-native",
            "model":"deepseek-flash","usage":{"source":"original-durable-observation"}});
        let ordinary:crate::llm::types::Message = serde_json::from_value(json!({
            "role":"assistant","content":"deepseek-history-body","provider_response":metadata,
            "reasoning_blocks":[reasoning.clone()],
            "assistant_blocks":[
                {"block_id":"ds-opaque-block","request_id":"canonical-ds-owner","sequence":0,"phase":"reasoning","delta":"","complete":true,
                 "provider":"deepseek","reasoning":reasoning,"provider_source":source(0,"thinking","deepseek-history-native:content:0")},
                {"block_id":"ds-text-block","request_id":"canonical-ds-owner","sequence":1,"phase":"commentary","delta":"deepseek-history-body","complete":true,
                 "provider":"deepseek","provider_source":source(1,"text","deepseek-history-native:content:1")}
            ],
            "tool_calls":[{"id":"opaque-history-call","type":"function","function":{"name":"history_tool_opaque","arguments":"{\"original\":true}"},
                "provider_source":source(2,"tool_use","opaque-history-call")}]
        })).map_err(err)?;
        let expected_metadata = ordinary
            .provider_response
            .clone()
            .ok_or_else(|| err("ordinary D1 missing"))?;
        let expected_reasoning = ordinary
            .reasoning_blocks
            .clone()
            .ok_or_else(|| err("original opaque reasoning missing"))?;
        let only = crate::llm::types::Message::provider_response_observation(serde_json::from_value(json!({
            "protocol":"anthropic","provider":"opaque-metadata-only-marker","request_id":"observation-only"
        })).map_err(err)?);
        let history = vec![
            crate::llm::types::Message::user("original opaque user".into()),
            only,
            ordinary,
            crate::llm::types::Message::tool_result(
                "opaque-history-call".into(),
                "history_tool_opaque".into(),
                "deepseek-tool-body".into(),
            ),
        ];
        let mut facts = Vec::new();
        for cold in [false, true] {
            let conversations = Arc::new(crate::memory::FileConversationStore::new(
                directory.join("conversations"),
            )?);
            let runtime = Arc::new(crate::state::FileRuntimeStateStore::new(
                directory.join("runtime"),
            )?);
            if cold {
                let restored = crate::memory::restore_messages(
                    &conversations.get_messages("root-deepseek-opaque").await?,
                )?;
                let original = restored
                    .iter()
                    .find(|message| {
                        message.content.as_text().as_deref() == Some("deepseek-history-body")
                    })
                    .ok_or_else(|| err("cold File owner lost original opaque history"))?;
                assert_eq!(
                    original.provider_response.as_ref(),
                    Some(&expected_metadata)
                );
                assert_eq!(
                    original.reasoning_blocks.as_ref(),
                    Some(&expected_reasoning),
                    "cold storage must preserve original Opaque variant/data/signature presence"
                );
            }
            let mut agent = ReactAgentBuilder::new()
                .configured_llm_client(client.clone(), config.clone())
                .model_streaming(streaming)
                .auto_project_rules(false)
                .build()?;
            agent.set_conversation_id("root-deepseek-opaque");
            agent.set_conversation_store(conversations.clone());
            agent.set_state_store(runtime.clone());
            let events = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                match agent
                    .chat_stream_message_with_invocation_context(
                        crate::llm::types::Message::user("actual opaque Root replay".into()),
                        CancellationToken::new(),
                        echo_core::agent::AgentInvocationContext {
                            history: (!cold).then(|| history.clone()),
                            ..Default::default()
                        },
                    )
                    .await
                {
                    Ok(stream) => stream.collect::<Vec<_>>().await,
                    Err(error) => vec![Err(error)],
                }
            })
            .await
            .map_err(err)?;
            let errors: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    Err(error) => Some(error.to_string()),
                    Ok(AgentEvent::Error { message, .. }) => Some(message.clone()),
                    _ => None,
                })
                .collect();
            let finals: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    Ok(AgentEvent::FinalAnswer(value)) => Some(value.clone()),
                    _ => None,
                })
                .collect();
            agent.force_checkpoint().await?;
            agent.close().await?;
            drop(agent);
            drop(conversations);
            drop(runtime);
            facts.push(json!({"cold":cold,"errors":errors,"finals":finals}));
        }
        let requests = client.requests.lock().map_err(err)?.clone();
        let report = json!({"streaming":streaming,"signature_present":signature_present,"actual_client_calls":requests.len(),"rounds":facts});
        std::fs::write(
            directory.join("child-result.json"),
            serde_json::to_vec_pretty(&report).map_err(err)?,
        )
        .map_err(err)?;
        eprintln!("ROOT_DEEPSEEK_OPAQUE_CHILD {report}");
        assert_eq!(
            requests.len(),
            2,
            "warm and new cold Agent must reach original configured client exactly once each: {report}"
        );
        for fact in &facts {
            assert_eq!(
                fact.get("errors"),
                Some(&json!([])),
                "actual Root failed: {report}"
            );
            assert_eq!(
                fact.get("finals"),
                Some(&json!(["OK"])),
                "actual Root final missing: {report}"
            );
        }
        for request in &requests {
            let original = request
                .messages
                .iter()
                .find(|message| {
                    message.content.as_text().as_deref() == Some("deepseek-history-body")
                })
                .ok_or_else(|| err("Root dropped original opaque body"))?;
            assert_eq!(
                original.provider_response.as_ref(),
                Some(&expected_metadata),
                "normal D1 must reach the actual adapter"
            );
            assert_eq!(
                original.reasoning_blocks.as_ref(),
                Some(&expected_reasoning),
                "opaque state must not become synthetic Signed or be duplicated"
            );
            assert!(
                request
                    .messages
                    .iter()
                    .all(|message| !message.is_provider_response_observation_only())
            );
            let saved = original
                .reasoning_blocks
                .as_ref()
                .and_then(|blocks| blocks.first())
                .ok_or_else(|| err("canonical opaque missing"))?;
            let crate::llm::types::ReasoningBlock::Opaque { provider, data, .. } = saved else {
                return Err(err("opaque state variant rewritten"));
            };
            assert_eq!(provider, "deepseek");
            let native: Value = serde_json::from_str(data).map_err(err)?;
            assert_eq!(
                native, opaque_native,
                "missing signature and explicit empty signature must remain distinct"
            );
        }
        return Ok(());
    }
    for streaming in [false, true] {
        for signature_present in [false, true] {
            let root = super::consumer_test_artifacts::root()?;
            let diagnostics = tempfile::Builder::new()
                .prefix("root-deepseek-opaque-")
                .tempdir_in(&root)
                .map_err(err)?
                .keep();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(err)?;
            let address = listener.local_addr().map_err(err)?;
            let endpoint = format!(
                "http://api.deepseek.com:{}/anthropic/v1/messages",
                address.port()
            );
            let proxy = format!("http://{address}");
            let wire = Arc::new(Mutex::new(Vec::<Value>::new()));
            let server_wire = wire.clone();
            let connections = Arc::new(AtomicUsize::new(0));
            let server_connections = connections.clone();
            let mut server = ServerAbort(tokio::spawn(async move {
                tokio::time::timeout(std::time::Duration::from_secs(10),async move {
                    for ordinal in 1..=2 {
                        let (mut socket,_)=listener.accept().await.map_err(err)?;
                        server_connections.fetch_add(1,Ordering::SeqCst);
                        let (line,request)=read_request(&mut socket).await.map_err(err)?;
                        if !line.contains("http://api.deepseek.com:") || !line.contains("/anthropic/") {
                            return Err(err(format!("official DeepSeek dialect identity was not preserved: {line}")));
                        }
                        server_wire.lock().map_err(err)?.push(request);
                        let payload=response_body(streaming,ordinal).replace("claude-sonnet-4-5","deepseek-flash");
                        let content_type=if streaming {"text/event-stream"} else {"application/json"};
                        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",payload.len()).as_bytes()).await.map_err(err)?;
                        socket.shutdown().await.map_err(err)?;
                    }
                    Ok(())
                }).await.map_err(err)?
            }));
            let module = module_path!()
                .split_once("::")
                .map(|(_, module)| module)
                .unwrap_or(module_path!());
            let mut command = tokio::process::Command::new(std::env::current_exe().map_err(err)?);
            let stdout_path = diagnostics.join("child.stdout.log");
            let stderr_path = diagnostics.join("child.stderr.log");
            command
                .args(["--exact", &format!("{module}::{NAME}"), "--nocapture"])
                .env("EKO_CONSUMER_TEST_ROOT", &root)
                .env(
                    "EKO_ROOT_DS_OPAQUE_CHILD_MODE",
                    if streaming { "stream" } else { "nonstream" },
                )
                .env(
                    "EKO_ROOT_DS_OPAQUE_SIGNATURE_PRESENT",
                    if signature_present { "yes" } else { "no" },
                )
                .env("EKO_ROOT_DS_OPAQUE_ENDPOINT", endpoint)
                .env("HTTP_PROXY", &proxy)
                .env("http_proxy", &proxy)
                .env("HTTPS_PROXY", &proxy)
                .env("https_proxy", &proxy)
                .env("NO_PROXY", "")
                .env("no_proxy", "")
                .env_remove("ALL_PROXY")
                .env_remove("all_proxy")
                .stdout(std::process::Stdio::from(
                    std::fs::File::create(&stdout_path).map_err(err)?,
                ))
                .stderr(std::process::Stdio::from(
                    std::fs::File::create(&stderr_path).map_err(err)?,
                ))
                .kill_on_drop(true);
            let mut child = command.spawn().map_err(err)?;
            let (status, wait_error) = match tokio::time::timeout(
                std::time::Duration::from_secs(15),
                child.wait(),
            )
            .await
            {
                Ok(Ok(status)) => (Some(status), None),
                Ok(Err(error)) => {
                    let _ = child.kill().await;
                    let status = child.wait().await.ok();
                    (status, Some(error.to_string()))
                }
                Err(error) => {
                    let _ = child.kill().await;
                    let status = child.wait().await.ok();
                    (status, Some(error.to_string()))
                }
            };
            let stdout = std::fs::read(&stdout_path).map_err(err)?;
            let stderr = std::fs::read(&stderr_path).map_err(err)?;
            let selected_one = String::from_utf8_lossy(&stdout).contains("running 1 test");
            let child_ok = status.as_ref().is_some_and(|status| status.success())
                && wait_error.is_none()
                && selected_one;
            let report = json!({"streaming":streaming,"signature_present":signature_present,"exit_code":status.as_ref().and_then(|status|status.code()),
                "wait_error":wait_error,"selected_one":selected_one,"http_before_server_join":wire.lock().map_err(err)?.len(),
                "connections_before_server_join":connections.load(Ordering::SeqCst),"stdout":stdout_path.display().to_string(),"stderr":stderr_path.display().to_string()});
            std::fs::write(
                diagnostics.join("result.json"),
                serde_json::to_vec_pretty(&report).map_err(err)?,
            )
            .map_err(err)?;
            eprintln!(
                "ROOT_DEEPSEEK_OPAQUE_PARENT {report}\nCHILD_STDOUT:\n{}\nCHILD_STDERR:\n{}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
            // 先保child首错，避免代理accept超时把真正Root/冷恢复错误覆盖。
            if !child_ok {
                server.0.abort();
                let _ = (&mut server.0).await;
                return Err(err(format!(
                    "opaque child first failure: {report}; {}",
                    String::from_utf8_lossy(&stderr)
                )));
            }
            (&mut server.0).await.map_err(err)??;
            let requests = wire.lock().map_err(err)?.clone();
            assert_eq!(
                requests.len(),
                2,
                "must observe actual warm and cold native HTTP: {report}"
            );
            assert_eq!(connections.load(Ordering::SeqCst), 2);
            for request in &requests {
                assert!(
                    !contains_key(request, "provider_response"),
                    "canonical D1 must not leak to native wire"
                );
                let all: Vec<_> = request
                    .get("messages")
                    .and_then(Value::as_array)
                    .ok_or_else(|| err("native messages missing"))?
                    .iter()
                    .filter_map(|message| message.get("content").and_then(Value::as_array))
                    .flatten()
                    .collect();
                let historical = request
                    .get("messages")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|message| message.get("content").and_then(Value::as_array))
                    .find(|blocks| {
                        blocks
                            .iter()
                            .any(|block| block.get("text") == Some(&json!("deepseek-history-body")))
                    })
                    .ok_or_else(|| err("original native opaque history missing"))?;
                assert_eq!(
                    historical
                        .iter()
                        .filter_map(|block| block.get("type").and_then(Value::as_str))
                        .collect::<Vec<_>>(),
                    vec!["thinking", "text", "tool_use"]
                );
                let thinking = historical
                    .first()
                    .ok_or_else(|| err("opaque native replay missing"))?;
                assert_eq!(thinking.get("thinking"), Some(&json!("opaque-original")));
                assert_eq!(
                    thinking.get("signature"),
                    signature_present.then(|| json!("")).as_ref(),
                    "signature omission and explicit empty value must not collapse"
                );
                assert_eq!(
                    all.iter()
                        .filter(|block| block.get("text") == Some(&json!("deepseek-history-body")))
                        .count(),
                    1
                );
                assert_eq!(
                    all.iter()
                        .filter(|block| block.get("id") == Some(&json!("opaque-history-call")))
                        .count(),
                    1
                );
                let results: Vec<_> = all
                    .iter()
                    .filter(|block| block.get("tool_use_id") == Some(&json!("opaque-history-call")))
                    .collect();
                assert_eq!(results.len(), 1, "original history tool result duplicated");
                assert_eq!(
                    results.first().and_then(|block| block.get("content")),
                    Some(&json!("deepseek-tool-body"))
                );
            }
        }
    }
    Ok(())
}
