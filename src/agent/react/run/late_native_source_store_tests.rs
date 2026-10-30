//! 真实 Responses SSE 经原 ReAct/Context/File 冷恢复后重放；不替换 Provider 或执行管线。
use crate::agent::{
    Agent, AgentEvent, AgentInvocationContext, CancellationToken, ReactAgentBuilder,
};
use crate::error::{ReactError, Result};
use crate::llm::types::{Message, ReasoningBlock};
use crate::llm::{
    ChatChunk, ChatRequest, ChatResponse, LlmApiProtocol, LlmClient, LlmConfig, ResponsesClient,
};
use crate::memory::ConversationStore;
use crate::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};
use echo_core::agent::{AgentOutputProjection, AssistantPhase};
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SCOPE: &str = "late-source-root-conversation";
const RUNTIME: &str = "late-source-root-runtime";
const NATIVE: &str = "late-actual-response";
const SUMMARY: &str = "原生摘要🙂";
const PLAIN: &str = "原生公开推理🧭";
const BODY: &str = "原生工具前正文";
const OPAQUE: &str = "opaque-original-unchanged==";
const ARGUMENTS: &str = r#"{"path":"真实🙂.rs","revision":7}"#;

fn error(message: impl ToString) -> ReactError {
    ReactError::Other(message.to_string())
}

// 只观察原请求，响应流直接委托真实 ResponsesClient；没有 canned ChatChunk。
struct RequestTap {
    inner: ResponsesClient,
    requests: Mutex<Vec<ChatRequest>>,
}
impl RequestTap {
    fn observe(&self, request: &ChatRequest) -> Result<()> {
        self.requests.lock().map_err(error)?.push(request.clone());
        Ok(())
    }
}
impl LlmClient for RequestTap {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        Box::pin(async move {
            self.observe(&request)?;
            self.inner.chat(request).await
        })
    }
    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>> {
        Box::pin(async move {
            self.observe(&request)?;
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

struct InspectTool(Arc<Mutex<Vec<Value>>>);
impl Tool for InspectTool {
    fn name(&self) -> &str {
        "late_source_inspect"
    }
    fn description(&self) -> &str {
        "记录真实工具边界收到的原参数"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"revision":{"type":"integer"}},"required":["path","revision"]})
    }
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }
    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            self.0
                .lock()
                .map_err(error)?
                .push(serde_json::to_value(parameters)?);
            Ok(ToolResult::success("original-inspection-result"))
        })
    }
}

fn original_response() -> Value {
    json!({"id":NATIVE,"status":"completed","output":[
        {"type":"reasoning","id":"late-reasoning","encrypted_content":OPAQUE,
         "summary":[{"type":"summary_text","text":SUMMARY}],
         "content":[{"type":"reasoning_text","text":PLAIN}]},
        {"type":"message","id":"late-message","role":"assistant","phase":"commentary",
         "content":[{"type":"output_text","text":BODY,"annotations":[]}]},
        {"type":"function_call","id":"late-function","call_id":"late-call",
         "name":"late_source_inspect","arguments":ARGUMENTS}
    ]})
}

fn event(value: Value) -> String {
    format!(
        "event: {}\ndata: {value}\n\n",
        value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )
}

fn response_body(ordinal: usize) -> String {
    if ordinal == 0 {
        // 首真 response.id 只在 completed 到达。item.done 已关闭 plaintext child，
        // 因而 e32 的 parent-before-source 排序会在真实消费者重复呈现整段 summary。
        return [
            event(json!({"type":"response.reasoning_summary_text.delta","item_id":"late-reasoning","output_index":0,"summary_index":0,"delta":SUMMARY})),
            event(json!({"type":"response.output_item.done","output_index":0,"item":{
                "type":"reasoning","id":"late-reasoning",
                "summary":[{"type":"summary_text","text":SUMMARY}],
                "content":[{"type":"reasoning_text","text":PLAIN}]}})),
            event(json!({"type":"response.output_item.done","output_index":1,"item":{
                "type":"message","id":"late-message","role":"assistant","phase":"commentary",
                "content":[{"type":"output_text","text":BODY,"annotations":[]}]}})),
            event(json!({"type":"response.output_item.added","output_index":2,"item":{
                "type":"function_call","id":"late-function","call_id":"late-call",
                "name":"late_source_inspect","arguments":""}})),
            event(json!({"type":"response.function_call_arguments.delta","item_id":"late-function","output_index":2,"delta":ARGUMENTS})),
            event(json!({"type":"response.function_call_arguments.done","item_id":"late-function","output_index":2,"arguments":ARGUMENTS})),
            event(json!({"type":"response.completed","response":original_response()})),
        ].concat();
    }
    event(json!({"type":"response.completed","response":{
    "id":format!("late-next-{ordinal}"),"status":"completed","output":[{
        "type":"message","id":format!("late-final-{ordinal}"),"role":"assistant",
        "content":[{"type":"output_text","text":"完成","annotations":[]}]
    }]}}))
}

async fn read_http(socket: &mut tokio::net::TcpStream) -> Result<Value> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let count = socket.read(&mut chunk).await?;
        if count == 0 || bytes.len() > 2_000_000 {
            return Err(error("incomplete loopback HTTP request"));
        }
        bytes.extend_from_slice(
            chunk
                .get(..count)
                .ok_or_else(|| error("invalid read count"))?,
        );
        let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
            continue;
        };
        let headers =
            String::from_utf8_lossy(bytes.get(..end).ok_or_else(|| error("missing headers"))?);
        let length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .ok_or_else(|| error("missing HTTP content length"))?;
        let start = end
            .checked_add(4)
            .ok_or_else(|| error("header offset overflow"))?;
        let stop = start
            .checked_add(length)
            .ok_or_else(|| error("body offset overflow"))?;
        if let Some(body) = bytes.get(start..stop) {
            return Ok(serde_json::from_slice(body)?);
        }
    }
}

struct Server {
    stop: CancellationToken,
    task: tokio::task::JoinHandle<Result<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}

fn native_message(messages: &[Message]) -> Result<&Message> {
    let selected = messages
        .iter()
        .filter(|message| {
            message
                .provider_response
                .as_ref()
                .and_then(|record| record.request_id.as_deref())
                == Some(NATIVE)
                && message
                    .tool_calls
                    .iter()
                    .flatten()
                    .any(|call| call.id == "late-call")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        selected.len(),
        1,
        "原响应必须由一条普通 Message 承载，不拆成额外 D1 行"
    );
    selected
        .first()
        .copied()
        .ok_or_else(|| error("native assistant Message missing"))
}

fn assert_original(message: &Message) -> Result<()> {
    assert_eq!(message.content.as_text().as_deref(), Some(BODY));
    assert!(!message.is_provider_response_observation_only());
    assert_eq!(
        message
            .provider_response
            .as_ref()
            .map(|record| record.protocol),
        Some(LlmApiProtocol::Responses)
    );
    let calls = message
        .tool_calls
        .as_deref()
        .ok_or_else(|| error("native tool missing"))?;
    assert_eq!(calls.len(), 1);
    let call = calls.first().ok_or_else(|| error("native call missing"))?;
    assert_eq!(call.id, "late-call");
    assert_eq!(call.function.name, "late_source_inspect");
    assert_eq!(call.function.arguments, ARGUMENTS);
    assert_eq!(
        call.provider_source
            .as_ref()
            .map(|source| source.request_id.as_str()),
        Some(NATIVE)
    );
    let blocks = message
        .assistant_blocks
        .as_deref()
        .ok_or_else(|| error("canonical blocks missing"))?;
    let prefix = blocks
        .iter()
        .find(|block| block.delta == SUMMARY && block.provider_source.is_none())
        .ok_or_else(|| error("真实早期 SourceNone summary 见证缺失"))?;
    let corrected = blocks
        .iter()
        .position(|block| {
            block.request_id == prefix.request_id
                && block.block_id == prefix.block_id
                && block.delta.is_empty()
                && block.provider_source.as_ref().is_some_and(|source| {
                    source.request_id == NATIVE && source.item_id == "late-reasoning"
                })
        })
        .ok_or_else(|| error("同一 canonical child 的 late source 修正缺失"))?;
    let parent = blocks
        .iter()
        .position(|block| {
            matches!(&block.reasoning,
        Some(ReasoningBlock::Opaque { id, data, summary, .. })
            if id == "late-reasoning" && data == OPAQUE && summary == &vec![SUMMARY.to_string()])
        })
        .ok_or_else(|| error("原 opaque parent 丢失或内容变化"))?;
    assert!(
        corrected < parent,
        "来源修正必须先于依赖此前缀的同chunk opaque parent"
    );
    assert!(
        blocks.windows(2).all(|pair| pair
            .first()
            .zip(pair.get(1))
            .is_some_and(|(a, b)| a.sequence < b.sequence)),
        "原响应全局 sequence 必须递增"
    );
    assert!(
        blocks
            .iter()
            .all(|block| block.request_id == prefix.request_id)
    );
    Ok(())
}

fn assert_render(events: &[AgentEvent], expected_reasoning: &str) -> Result<()> {
    let mut projection = AgentOutputProjection::default();
    let mut reasoning = String::new();
    let mut finals = Vec::new();
    for event in events {
        match event {
            AgentEvent::ExecutionStatus(status) => {
                projection.observe_status(status)?;
            }
            AgentEvent::AssistantMessage { block } => {
                if let Some(delta) = projection.observe_block(block)?
                    && block.phase == AssistantPhase::Reasoning
                {
                    reasoning.push_str(&delta.text);
                }
            }
            AgentEvent::FinalAnswer(answer) => {
                projection.confirm_answer(answer)?;
                finals.push(answer.as_str());
            }
            _ => {}
        }
    }
    assert_eq!(
        reasoning, expected_reasoning,
        "同一公开 summary/plaintext 只能呈现一次"
    );
    assert_eq!(finals, vec!["完成"]);
    assert_eq!(projection.finish()?, "完成");
    Ok(())
}

fn contains_key(value: &Value, name: &str) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key(name) || map.values().any(|value| contains_key(value, name))
        }
        Value::Array(values) => values.iter().any(|value| contains_key(value, name)),
        _ => false,
    }
}

fn text_occurrences(value: &Value, expected: &str) -> usize {
    match value {
        Value::String(text) => text.matches(expected).count(),
        Value::Array(values) => values
            .iter()
            .map(|value| text_occurrences(value, expected))
            .fold(0_usize, usize::saturating_add),
        Value::Object(map) => map
            .values()
            .map(|value| text_occurrences(value, expected))
            .fold(0_usize, usize::saturating_add),
        _ => 0,
    }
}

fn assert_wire(body: &Value) -> Result<()> {
    let input = body
        .get("input")
        .and_then(Value::as_array)
        .ok_or_else(|| error("HTTP input missing"))?;
    let actual = input
        .iter()
        .filter(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| ["late-reasoning", "late-message", "late-function"].contains(&id))
        })
        .cloned()
        .collect::<Vec<_>>();
    // 扫整个 input，不能只挑已知 ID 后漏掉额外无 ID 或另取 ID 的副本。
    let input_value = body
        .get("input")
        .ok_or_else(|| error("HTTP input missing"))?;
    for original in [SUMMARY, PLAIN, BODY, OPAQUE, ARGUMENTS] {
        assert_eq!(
            text_occurrences(input_value, original),
            1,
            "原文/opaque/参数在全部 HTTP input 中必须各一次: {original}"
        );
    }
    assert_eq!(
        input
            .iter()
            .filter(
                |item| item.get("type").and_then(Value::as_str) == Some("function_call")
                    && item.get("call_id").and_then(Value::as_str) == Some("late-call")
            )
            .count(),
        1,
        "不能用另一个或缺失 item ID 重复同一原 function_call"
    );
    assert_eq!(
        Value::Array(actual),
        original_response()
            .get("output")
            .cloned()
            .ok_or_else(|| error("fixture output missing"))?,
        "原 summary/plaintext/opaque/body/args 各一次且原顺序不变"
    );
    assert_eq!(
        input
            .iter()
            .filter(
                |item| item.get("type").and_then(Value::as_str) == Some("function_call_output")
                    && item.get("call_id").and_then(Value::as_str) == Some("late-call")
            )
            .count(),
        1
    );
    for field in [
        "provider_response",
        "provider_source",
        "assistant_blocks",
        "approved_assistant_blocks",
        "runtime_state_id",
        "transcript_generation_id",
    ] {
        assert!(
            !contains_key(body, field),
            "内部字段不得泄漏到 vendor HTTP: {field}"
        );
    }
    Ok(())
}

async fn round(
    config: &LlmConfig,
    directory: &std::path::Path,
    effects: Arc<Mutex<Vec<Value>>>,
    cold: bool,
) -> Result<(Vec<AgentEvent>, Vec<Message>, Vec<ChatRequest>)> {
    let conversations = Arc::new(crate::memory::FileConversationStore::new(
        directory.join("conversations"),
    )?);
    let runtime = Arc::new(crate::state::FileRuntimeStateStore::new(
        directory.join("runtime"),
    )?);
    let tap = Arc::new(RequestTap {
        inner: ResponsesClient::new(config.clone())?,
        requests: Mutex::new(Vec::new()),
    });
    let agent = ReactAgentBuilder::new()
        .configured_llm_client(tap.clone(), config.clone())
        .model_streaming(true)
        .auto_project_rules(false)
        .conversation_id(SCOPE)
        .conversation_store(conversations.clone())
        .state_store(runtime.clone())
        .tool(Box::new(InspectTool(effects)))
        .build()?;
    let invocation = AgentInvocationContext {
        runtime_state_id: Some(RUNTIME.into()),
        transcript_generation_id: Some(RUNTIME.into()),
        runtime: Some(echo_core::tools::ExternalRunContext {
            conversation_id: Some(SCOPE.into()),
            run_id: Some(
                if cold {
                    "late-cold-run"
                } else {
                    "late-original-run"
                }
                .into(),
            ),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = async {
        let stream = agent
            .chat_stream_message_with_invocation_context(
                Message::user(
                    if cold {
                        "冷恢复后继续"
                    } else {
                        "执行原始检查"
                    }
                    .into(),
                ),
                CancellationToken::new(),
                invocation,
            )
            .await?;
        // 收完原 source 到 EOF；不以 tool/final/错误事件代替物理结算。
        let raw = tokio::time::timeout(Duration::from_secs(10), stream.collect::<Vec<_>>())
            .await
            .map_err(error)?;
        let events = raw.into_iter().collect::<Result<Vec<_>>>()?;
        let messages = agent.context().lock().await.messages().to_vec();
        let requests = tap.requests.lock().map_err(error)?.clone();
        Ok((events, messages, requests))
    }
    .await;
    let closed = agent.close().await;
    drop(agent);
    drop(runtime);
    drop(conversations);
    drop(tap);
    closed?;
    result
}

async fn reject_foreign_observation(config: &LlmConfig, history: &[Message]) -> Result<()> {
    // 只篡改实际 Provider/File 已产出的一个 D1 作为反控，不手造成功响应或本地重放器。
    let mut history = history.to_vec();
    let original = history
        .iter_mut()
        .find(|message| {
            message
                .provider_response
                .as_ref()
                .and_then(|record| record.request_id.as_deref())
                == Some(NATIVE)
                && message
                    .tool_calls
                    .as_ref()
                    .is_some_and(|calls| !calls.is_empty())
        })
        .ok_or_else(|| error("actual cold Message missing for conflict control"))?;
    original
        .provider_response
        .as_mut()
        .ok_or_else(|| error("original D1 missing"))?
        .request_id = Some("foreign-actual-response".into());
    let tap = Arc::new(RequestTap {
        inner: ResponsesClient::new(config.clone())?,
        requests: Mutex::new(Vec::new()),
    });
    let agent = ReactAgentBuilder::new()
        .configured_llm_client(tap.clone(), config.clone())
        .model_streaming(true)
        .auto_project_rules(false)
        .build()?;
    let result = async {
        let stream = agent
            .chat_stream_message_with_invocation_context(
                Message::user("拒绝与实际来源冲突的观察".into()),
                CancellationToken::new(),
                AgentInvocationContext {
                    history: Some(history),
                    ..Default::default()
                },
            )
            .await?;
        let raw = tokio::time::timeout(Duration::from_secs(10), stream.collect::<Vec<_>>())
            .await
            .map_err(error)?;
        assert!(
            !raw.iter()
                .any(|event| matches!(event, Ok(AgentEvent::FinalAnswer(_))))
        );
        assert!(
            raw.iter().any(|event| match event {
                Err(error) =>
                    error.to_string().contains("Responses") && error.to_string().contains("replay"),
                Ok(AgentEvent::Error { message, .. }) =>
                    message.contains("Responses") && message.contains("replay"),
                _ => false,
            }),
            "必须由原 adapter 来源校验拒绝，不能用 setup/持久化失败代替"
        );
        let requests = tap.requests.lock().map_err(error)?;
        assert_eq!(requests.len(), 1);
        let request = requests
            .first()
            .ok_or_else(|| error("original configured client was not reached"))?;
        assert!(
            request.messages.iter().any(|message| message
                .provider_response
                .as_ref()
                .and_then(|record| record.request_id.as_deref())
                == Some("foreign-actual-response")),
            "Root 不得先清 D1 隐藏 actual-ID 冲突"
        );
        Ok(())
    }
    .await;
    let closed = agent.close().await;
    closed?;
    result
}

#[tokio::test]
async fn responses_late_native_source_survives_react_file_cold_and_next_http_once() -> Result<()> {
    let base = std::env::var_os("EKO_CONSUMER_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| error("EKO_CONSUMER_TEST_ROOT must point inside the main workspace"))?;
    std::fs::create_dir_all(&base)?;
    let directory = tempfile::Builder::new()
        .prefix("late-native-root-")
        .tempdir_in(base)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let config = LlmConfig::for_provider(
        "openai",
        format!("http://{}/v1/responses", listener.local_addr()?),
        "fixture-only-not-live",
        "gpt-4.1",
        LlmApiProtocol::Responses,
    )?;
    let wire = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server_wire = wire.clone();
    let stop = CancellationToken::new();
    let server_stop = stop.clone();
    let mut server = Server {
        stop,
        task: tokio::spawn(async move {
            loop {
                let (mut socket, _) = tokio::select! {
                    _ = server_stop.cancelled() => return Ok(()),
                    accepted = listener.accept() => accepted?,
                };
                let request = tokio::time::timeout(Duration::from_secs(5), read_http(&mut socket))
                    .await
                    .map_err(error)??;
                assert_eq!(request.get("stream").and_then(Value::as_bool), Some(true));
                let ordinal = {
                    let mut requests = server_wire.lock().map_err(error)?;
                    let ordinal = requests.len();
                    requests.push(request);
                    ordinal
                };
                let body = response_body(ordinal);
                // Content-Length 是 HTTP 字节长度，绝不按字节截断文本。
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
                socket.shutdown().await?;
            }
        }),
    };
    let effects = Arc::new(Mutex::new(Vec::new()));
    let result = async {
        let (events, context, requests) = round(&config, directory.path(), effects.clone(), false).await?;
        // 首轮所有 Agent/RuntimeStore/ConversationStore/client handles 已关；此后仅从 File 重开。
        assert_render(&events, &format!("{SUMMARY}{PLAIN}"))?;
        assert_eq!(requests.len(), 2, "真实工具后必须仅再请求一次模型");
        let original = native_message(&context)?;
        assert_original(original)?;
        let context_blocks = original.assistant_blocks.as_ref().ok_or_else(|| error("original blocks missing"))?;
        for block in context_blocks {
            assert!(events.iter().any(|event| matches!(event, AgentEvent::AssistantMessage { block: actual } if actual == block)), "ReAct collector 不得改写已发 canonical request/block/sequence");
        }
        let restored = {
            let cold = crate::memory::FileConversationStore::new(directory.path().join("conversations"))?;
            crate::memory::restore_messages(&cold.get_messages(SCOPE).await?)?
        };
        let stored = native_message(&restored)?;
        assert_original(stored)?;
        assert_eq!(serde_json::to_value(stored)?, serde_json::to_value(original)?, "原 Context→File 冷读 Message 必须无损");
        let (cold_events, cold_context, cold_requests) = round(&config, directory.path(), effects.clone(), true).await?;
        assert_render(&cold_events, "")?;
        assert_eq!(cold_requests.len(), 1, "冷恢复不得重执行历史工具或多跑模型");
        assert_eq!(serde_json::to_value(native_message(&cold_context)?)?, serde_json::to_value(stored)?);
        for request in requests.iter().skip(1).chain(cold_requests.iter()) {
            assert_eq!(serde_json::to_value(native_message(&request.messages)?)?, serde_json::to_value(stored)?, "Root 下一请求保留普通 D1 与全部 canonical 原件到 adapter 边界");
        }
        assert_eq!(*effects.lock().map_err(error)?, vec![serde_json::from_str::<Value>(ARGUMENTS)?]);
        reject_foreign_observation(&config, &restored).await?;
        let actual_wire = wire.lock().map_err(error)?.clone();
        assert_eq!(actual_wire.len(), 3, "冲突必须在真实 HTTP 连接前拒绝");
        for request in actual_wire.iter().skip(1) { assert_wire(request)?; }
        Ok(())
    }.await;
    server.stop.cancel();
    let settled = tokio::time::timeout(Duration::from_secs(5), &mut server.task)
        .await
        .map_err(error)?
        .map_err(error)?;
    settled?;
    result
}
