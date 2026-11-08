//! 原生消息合同的执行回归：只驱动既有 ReAct，不建立第二执行器。
//! provider fixture 同时携带旧 content 投影和 canonical blocks，暴露丢身份/阶段的接缝。

use super::types::{StreamInit, StreamMode};
use crate::agent::{Agent, AgentEvent, CancellationToken, ReactAgent, ReactAgentBuilder};
use crate::error::{ReactError, Result};
use crate::llm::types::{DeltaMessage, Message, ReasoningBlock};
use crate::testing::{MockLlmClient, StreamChunk};
use crate::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};
use echo_core::agent::{AssistantMessageBlock, AssistantPhase, Critic, Critique};
use echo_core::guard::{Guard, GuardDirection, GuardResult};
use echo_core::memory::ConversationStore;
use futures::{StreamExt, future::BoxFuture};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

fn block(
    id: &str,
    request: &str,
    sequence: u64,
    phase: AssistantPhase,
    text: &str,
) -> AssistantMessageBlock {
    AssistantMessageBlock {
        block_id: id.to_string(),
        request_id: request.to_string(),
        sequence,
        phase,
        delta: text.to_string(),
        complete: true,
        provider: Some("contract_fixture".to_string()),
        provider_source: None,
        reasoning: None,
    }
}

fn native_delta(blocks: Vec<AssistantMessageBlock>, calls: Value) -> Result<StreamChunk> {
    let text: String = blocks
        .iter()
        .filter(|b| b.phase != AssistantPhase::Reasoning)
        .map(|b| b.delta.as_str())
        .collect();
    let reasoning: Vec<_> = blocks.iter().filter_map(|b| b.reasoning.clone()).collect();
    // 使用真实 DeltaMessage 的反序列化入口；旧类型静默丢 assistant_blocks 时应由执行断言报 RED。
    let delta: DeltaMessage = serde_json::from_value(json!({
        "role":"assistant", "content":text, "assistant_blocks":blocks,
        "reasoning_blocks":reasoning, "tool_calls":calls
    }))?;
    Ok(StreamChunk::Delta(Box::new(delta)))
}

fn terminal(reason: &str) -> StreamChunk {
    StreamChunk::Terminal {
        finish_reason: Some(reason.to_string()),
        usage: None,
    }
}

fn call(index: u32, id: &str, name: &str) -> Value {
    json!({"index":index,"id":id,"type":"function","function":{"name":name,"arguments":"{}"}})
}

fn mock_agent(llm: Arc<MockLlmClient>) -> Result<ReactAgent> {
    let mut agent = ReactAgentBuilder::new()
        .llm_client(llm)
        .system_prompt("按消息协议执行，不猜阶段。")
        .max_iterations(6)
        .build()?;
    agent.config_mut().llm_max_retries = 0;
    agent.config_mut().llm_retry_delay_ms = 0;
    Ok(agent)
}

async fn collect_raw(agent: &ReactAgent, mode: StreamMode) -> Result<Vec<Result<AgentEvent>>> {
    let stream = agent
        .run_stream_channel(
            StreamInit {
                text: "执行合同场景 🧪".to_string(),
                message: None,
                label: String::new(),
                invocation: None,
            },
            mode,
        )
        .await?;
    tokio::time::timeout(Duration::from_secs(5), stream.collect())
        .await
        .map_err(|_| ReactError::Other("native execution fixture timed out".to_string()))
}

async fn collect(agent: &ReactAgent) -> Result<Vec<AgentEvent>> {
    let events: Vec<AgentEvent> = collect_raw(agent, StreamMode::Chat)
        .await?
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    assert_no_legacy_assistant_events(&events);
    Ok(events)
}
fn assert_no_legacy_assistant_events(events: &[AgentEvent]) {
    // 校验对外事件形状，旧Rust变体退役后仍防止重新发布旧wire副本。
    assert!(
        events.iter().all(|event| {
            serde_json::to_value(event).is_ok_and(|value| {
                value
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| !matches!(kind, "token" | "think_start" | "think_end"))
            })
        }),
        "当前执行不得发布旧Token/Think wire副本"
    );
}

fn answers(events: &[AgentEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::FinalAnswer(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn blocks(events: &[AgentEvent]) -> Vec<&AssistantMessageBlock> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::AssistantMessage { block } => Some(block),
            _ => None,
        })
        .collect()
}

fn visible_text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::FinalAnswer(t) => Some(t.as_str()),
            AgentEvent::AssistantMessage { block } if block.phase != AssistantPhase::Reasoning => {
                Some(block.delta.as_str())
            }
            _ => None,
        })
        .collect()
}

fn status_values(events: &[AgentEvent]) -> Result<Vec<Value>> {
    // 不依赖尚在合流的 enum facade 形状；校验真实发布的序列化事件合同。
    Ok(events
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("execution_status"))
        .collect())
}

fn assert_one_settled_execution(events: &[AgentEvent]) -> Result<()> {
    let statuses = status_values(events)?;
    let settled: Vec<_> = statuses
        .iter()
        .filter_map(|e| e.get("data"))
        .filter(|e| e.get("status").and_then(Value::as_str) == Some("settled"))
        .collect();
    assert_eq!(
        settled.len(),
        1,
        "成功必须有且仅有一次物理执行结算：{statuses:?}"
    );
    let identities: std::collections::HashSet<_> = statuses
        .iter()
        .filter_map(|e| e.pointer("/data/execution_id").and_then(Value::as_str))
        .collect();
    assert_eq!(identities.len(), 1, "模型请求/Stop继续不能另开执行身份");
    assert!(!identities.contains(""));
    let started = statuses
        .iter()
        .filter(|e| e.pointer("/data/status").and_then(Value::as_str) == Some("started"))
        .count();
    assert_eq!(started, 1, "同一执行Started只发一次");
    let sequences: Vec<_> = statuses
        .iter()
        .filter_map(|e| e.pointer("/data/sequence").and_then(Value::as_u64))
        .collect();
    assert_eq!(sequences.len(), statuses.len());
    assert!(
        sequences
            .windows(2)
            .all(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a, b)| a < b)),
        "运行状态序号严格递增：{sequences:?}"
    );
    let settled_position = events.iter().position(|e| matches!(e,AgentEvent::ExecutionStatus(status) if status.status==echo_core::agent::ExecutionStatusKind::Settled));
    let final_position = events
        .iter()
        .position(|e| matches!(e, AgentEvent::FinalAnswer(_)));
    assert!(
        settled_position
            .zip(final_position)
            .is_some_and(|(s, f)| f < s),
        "最终内容先发布，物理结算status最后发布"
    );
    assert!(
        matches!(events.last(),Some(AgentEvent::ExecutionStatus(status)) if status.status==echo_core::agent::ExecutionStatusKind::Settled),
        "terminal status必须是最后事件，SDK此处可安全关流"
    );
    Ok(())
}

#[tokio::test]
async fn native_message_plain_text_success_has_one_runtime_final() -> Result<()> {
    let llm = Arc::new(MockLlmClient::new().with_response("完成：中文与 emoji 🦉"));
    let agent = mock_agent(llm.clone())?;
    let events = collect(&agent).await?;
    assert_eq!(answers(&events), vec!["完成：中文与 emoji 🦉"]);
    assert_eq!(llm.call_count(), 1);
    assert_one_settled_execution(&events)
}

#[tokio::test]
async fn native_message_nonstream_public_execute_uses_same_terminal_path() -> Result<()> {
    let llm = Arc::new(MockLlmClient::new().with_response("非流式完成"));
    let agent = mock_agent(llm.clone())?;
    assert_eq!(agent.execute("run").await?, "非流式完成");
    assert_eq!(llm.call_count(), 1);
    Ok(())
}

async fn native_transport_http_request(socket: &mut tokio::net::TcpStream) -> Result<Value> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let count = socket.read(&mut chunk).await?;
        if count == 0 || bytes.len() > 2_000_000 {
            return Err(std::io::Error::other("incomplete native transport HTTP request").into());
        }
        bytes.extend_from_slice(chunk.get(..count).unwrap_or_default());
        let Some(header_end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(bytes.get(..header_end).unwrap_or_default());
        let length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .ok_or_else(|| std::io::Error::other("missing native transport content length"))?;
        let body_start = header_end.saturating_add(4);
        if let Some(body) = bytes.get(body_start..body_start.saturating_add(length)) {
            return Ok(serde_json::from_slice(body)?);
        }
    }
}

#[tokio::test]
async fn native_message_explicit_model_transport_preserves_return_form_and_contexts() -> Result<()>
{
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let config = crate::llm::LlmConfig::for_provider(
        "native-local-http",
        format!("http://{}/v1/chat/completions", listener.local_addr()?),
        "fixture-key",
        "fixture-model",
        crate::llm::LlmApiProtocol::ChatCompletions,
    )?;
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async move {
            let mut requests = Vec::new();
            for ordinal in 0..6 {
                let (mut socket, _) = listener.accept().await?;
                let request = native_transport_http_request(&mut socket).await?;
                let answer = format!("HTTP-ANSWER-{ordinal}");
                let (content_type, body) = if request.get("stream").and_then(Value::as_bool) == Some(true) {
                    let delta = serde_json::json!({"id":format!("http-{ordinal}"),"choices":[{"index":0,"delta":{"role":"assistant","content":answer},"finish_reason":null}]});
                    let stop = serde_json::json!({"id":format!("http-{ordinal}"),"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
                    ("text/event-stream", format!("data: {delta}\n\ndata: {stop}\n\ndata: [DONE]\n\n"))
                } else {
                    ("application/json", serde_json::json!({"id":format!("http-{ordinal}"),"choices":[{"index":0,"message":{"role":"assistant","content":answer},"finish_reason":"stop"}]}).to_string())
                };
                eprintln!("NATIVE_TRANSPORT_FIXTURE ordinal={ordinal} request={request} response={body:?}");
                // HTTP Content-Length 按字节计算；这里没有按字节截断文本。
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).await?;
                requests.push(request);
            }
            Ok::<_, ReactError>(requests)
        }).await.map_err(|error| ReactError::Other(error.to_string()))?
    });
    let execute = ReactAgentBuilder::new()
        .llm_config(config.clone())
        .model_streaming(false)
        .auto_project_rules(false)
        .build()?;
    assert_eq!(execute.execute("EXECUTE-FIRST").await?, "HTTP-ANSWER-0");
    assert_eq!(execute.execute("EXECUTE-SECOND").await?, "HTTP-ANSWER-1");
    let chat = ReactAgentBuilder::new()
        .llm_config(config.clone())
        .model_streaming(false)
        .auto_project_rules(false)
        .build()?;
    assert_eq!(chat.chat("CHAT-FIRST").await?, "HTTP-ANSWER-2");
    assert_eq!(chat.chat("CHAT-SECOND").await?, "HTTP-ANSWER-3");
    let events = chat
        .execute_stream("STREAM-POSITIVE")
        .await?
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(answers(&events), vec!["HTTP-ANSWER-4"]);
    assert_one_settled_execution(&events)?;
    let default = ReactAgentBuilder::new()
        .llm_config(config)
        .auto_project_rules(false)
        .build()?;
    assert_eq!(
        default.execute("DEFAULT-STRING-ENTRY").await?,
        "HTTP-ANSWER-5"
    );
    let requests = server
        .await
        .map_err(|error| ReactError::Other(error.to_string()))??;
    assert_eq!(requests.len(), 6);
    assert!(
        requests
            .iter()
            .take(5)
            .all(|request| request.get("stream").and_then(Value::as_bool) != Some(true)),
        "显式非流式模型传输不能因公开返回形式被改成stream=true"
    );
    assert_eq!(
        requests
            .last()
            .and_then(|request| request.get("stream"))
            .and_then(Value::as_bool),
        Some(true)
    );
    let has_text = |request: &Value, expected: &str| {
        request
            .get("messages")
            .and_then(Value::as_array)
            .is_some_and(|messages| {
                messages
                    .iter()
                    .any(|message| message.get("content").and_then(Value::as_str) == Some(expected))
            })
    };
    let second_execute = requests
        .get(1)
        .ok_or_else(|| ReactError::Other("missing second execute request".into()))?;
    assert!(!has_text(second_execute, "EXECUTE-FIRST"));
    assert!(!has_text(second_execute, "HTTP-ANSWER-0"));
    assert!(has_text(second_execute, "EXECUTE-SECOND"));
    let second_chat = requests
        .get(3)
        .ok_or_else(|| ReactError::Other("missing second chat request".into()))?;
    assert!(has_text(second_chat, "CHAT-FIRST"));
    assert!(has_text(second_chat, "HTTP-ANSWER-2"));
    assert!(has_text(second_chat, "CHAT-SECOND"));
    Ok(())
}

async fn progress_case(phase: AssistantPhase) -> Result<()> {
    let first = block("progress-1", "response-progress", 0, phase, "正在核对 🧪");
    let final_block = block(
        "final-2",
        "response-final",
        0,
        AssistantPhase::FinalAnswer,
        "已核对完成",
    );
    let llm = Arc::new(
        MockLlmClient::new()
            .with_stream_script(vec![
                native_delta(vec![first.clone()], Value::Null)?,
                terminal("stop"),
            ])
            .with_stream_script(vec![
                native_delta(vec![final_block], Value::Null)?,
                terminal("stop"),
            ]),
    );
    let agent = mock_agent(llm.clone())?;
    let events = collect(&agent).await?;
    assert_eq!(llm.call_count(), 2, "显式进度/部分回答不能当执行终态");
    assert_eq!(answers(&events), vec!["已核对完成"]);
    assert!(
        blocks(&events).contains(&&first),
        "进度保 provider 身份、phase、sequence"
    );
    let calls = llm.all_calls();
    assert!(
        calls
            .get(1)
            .into_iter()
            .flatten()
            .flat_map(|m| m.assistant_blocks.iter().flatten())
            .any(|b| b == &first),
        "下一请求必须看到原进度"
    );
    assert_one_settled_execution(&events)
}

#[tokio::test]
async fn native_message_commentary_only_continues_same_execution() -> Result<()> {
    progress_case(AssistantPhase::Commentary).await
}
#[tokio::test]
async fn native_message_partial_only_continues_same_execution() -> Result<()> {
    progress_case(AssistantPhase::PartialAnswer).await
}

struct EffectTool {
    name: &'static str,
    effects: Arc<AtomicUsize>,
    delay: Duration,
}
impl Tool for EffectTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Count actual executions at the existing Tool boundary"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{}})
    }
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }
    fn allows_parallel_batch_execution(&self) -> bool {
        true
    }
    fn execute(&self, _params: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            self.effects.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            Ok(ToolResult::success(format!("{} result", self.name)))
        })
    }
}

#[tokio::test]
async fn native_message_final_with_tool_executes_tool_then_requests_again() -> Result<()> {
    let draft = block(
        "early-final",
        "response-tool",
        0,
        AssistantPhase::FinalAnswer,
        "工具前的原生final",
    );
    let llm = Arc::new(
        MockLlmClient::new()
            .with_stream_script(vec![
                native_delta(vec![draft.clone()], json!([call(0, "effect-1", "effect")]))?,
                terminal("tool_calls"),
            ])
            .with_response("工具完成后的最终结果"),
    );
    let effects = Arc::new(AtomicUsize::new(0));
    let agent = ReactAgentBuilder::new()
        .llm_client(llm.clone())
        .tool(Box::new(EffectTool {
            name: "effect",
            effects: effects.clone(),
            delay: Duration::ZERO,
        }))
        .build()?;
    let events = collect(&agent).await?;
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(llm.call_count(), 2);
    assert_eq!(answers(&events), vec!["工具完成后的最终结果"]);
    assert!(
        !blocks(&events).contains(&&draft),
        "与工具共存的Final候选不能借中间公开分支泄露"
    );
    assert!(!visible_text(&events).contains(draft.delta.as_str()));
    assert!(
        !blocks(&events)
            .iter()
            .any(|b| b.phase == AssistantPhase::Reasoning && b.delta.contains("工具前"))
    );
    let result_pos = events
        .iter()
        .position(|e| matches!(e,AgentEvent::ToolResult{call_id,..} if call_id=="effect-1"));
    let final_pos = events
        .iter()
        .position(|e| matches!(e, AgentEvent::FinalAnswer(_)));
    assert!(result_pos.zip(final_pos).is_some_and(|(r, f)| r < f));
    let calls = llm.all_calls();
    assert!(
        calls
            .get(1)
            .into_iter()
            .flatten()
            .any(|m| m.tool_call_id.as_deref() == Some("effect-1"))
    );
    let raw = calls
        .get(1)
        .into_iter()
        .flatten()
        .find(|message| message.content.as_text().as_deref() == Some(draft.delta.as_str()))
        .ok_or_else(|| ReactError::Other("original final/tool candidate history missing".into()))?;
    assert_eq!(raw.assistant_blocks.as_deref(), Some([draft].as_slice()));
    assert_eq!(
        raw.approved_assistant_blocks.as_deref(),
        Some([].as_slice())
    );
    assert_one_settled_execution(&events)
}

fn provider_source(
    item: &str,
    output_index: u32,
    kind: &str,
    phase: Option<&str>,
) -> crate::llm::types::ProviderOutputSource {
    crate::llm::types::ProviderOutputSource {
        protocol: crate::llm::LlmApiProtocol::Responses,
        request_id: "response-multi".into(),
        item_id: item.into(),
        output_index,
        content_index: if kind == "function_call" {
            None
        } else {
            Some(0)
        },
        content_kind: Some(kind.into()),
        phase: phase.map(str::to_string),
    }
}

#[tokio::test]
async fn native_message_multiple_messages_and_parallel_tools_keep_order_and_identity() -> Result<()>
{
    let dir = fixture_dir()?;
    let expected: Vec<_> = [
        (
            "m1",
            0,
            AssistantPhase::Commentary,
            "先检查",
            "output_text",
            Some("commentary"),
        ),
        (
            "m2",
            2,
            AssistantPhase::PartialAnswer,
            "已知部分",
            "refusal",
            Some("partial_answer"),
        ),
        (
            "m3",
            4,
            AssistantPhase::Unknown,
            "未给阶段",
            "output_text",
            Some("provider_future_phase"),
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(sequence, (id, index, phase, text, kind, wire_phase))| {
        let mut message = block(
            id,
            "response-multi",
            u64::try_from(sequence).unwrap_or(u64::MAX),
            phase,
            text,
        );
        message.provider_source = Some(provider_source(id, index, kind, wire_phase));
        message
    })
    .collect();
    let slow_source = provider_source("decl-slow", 1, "function_call", None);
    let fast_source = provider_source("decl-fast", 3, "function_call", None);
    let calls = json!([
        {"index":1,"id":"slow-call","type":"function","function":{"name":"slow","arguments":"{}"},"provider_source":slow_source},
        {"index":3,"id":"fast-call","type":"function","function":{"name":"fast","arguments":"{}"},"provider_source":fast_source}
    ]);
    let llm = Arc::new(
        MockLlmClient::new()
            .with_stream_script(vec![
                native_delta(expected.clone(), calls)?,
                terminal("tool_calls"),
            ])
            .with_response("两项都已完成"),
    );
    let effects = Arc::new(AtomicUsize::new(0));
    let conversations = Arc::new(crate::memory::FileConversationStore::new(dir.path())?);
    let agent = ReactAgentBuilder::new()
        .llm_client(llm.clone())
        .conversation_id("native-source-order")
        .conversation_store(conversations.clone())
        .state_store(Arc::new(crate::state::FileRuntimeStateStore::new(
            dir.path(),
        )?))
        .tool(Box::new(EffectTool {
            name: "slow",
            effects: effects.clone(),
            delay: Duration::from_millis(30),
        }))
        .tool(Box::new(EffectTool {
            name: "fast",
            effects: effects.clone(),
            delay: Duration::ZERO,
        }))
        .build()?;
    let events = collect(&agent).await?;
    let observed: Vec<_> = blocks(&events)
        .into_iter()
        .filter(|block| block.request_id == "response-multi")
        .cloned()
        .collect();
    assert_eq!(
        observed, expected,
        "多个消息保原native身份/phase/refusal kind，不能按tool改文字类型"
    );
    let results: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(results, vec!["slow-call", "fast-call"]);
    assert_eq!(effects.load(Ordering::SeqCst), 2);
    let model_calls = llm.all_calls();
    let second = model_calls
        .get(1)
        .ok_or_else(|| ReactError::Other("post-tool request missing".into()))?;
    let declaration = second
        .iter()
        .find(|message| {
            message
                .tool_calls
                .as_ref()
                .is_some_and(|calls| calls.iter().any(|call| call.id == "slow-call"))
        })
        .ok_or_else(|| ReactError::Other("original tool declaration missing".into()))?;
    let declared = declaration
        .tool_calls
        .as_deref()
        .ok_or_else(|| ReactError::Other("tool declarations missing".into()))?;
    let sources: Vec<_> = declared
        .iter()
        .filter_map(|call| call.provider_source.clone())
        .collect();
    assert_eq!(
        sources,
        vec![slow_source, fast_source],
        "DeltaToolCall→原parser→Message→下一request不能丢声明来源"
    );
    let mut original_order: Vec<_> = declaration
        .assistant_blocks
        .iter()
        .flatten()
        .filter_map(|block| block.provider_source.clone())
        .chain(sources.clone())
        .collect();
    original_order.sort_by_key(|source| source.output_index);
    assert_eq!(
        original_order
            .iter()
            .map(|source| (
                source.output_index,
                source.item_id.as_str(),
                source.content_kind.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![
            (0, "m1", Some("output_text")),
            (1, "decl-slow", Some("function_call")),
            (2, "m2", Some("refusal")),
            (3, "decl-fast", Some("function_call")),
            (4, "m3", Some("output_text"))
        ],
        "断言的是模型原文字/工具声明交错，独立于实际工具开始时间"
    );
    // ToolCall事件是经过策略后的真实执行开始；用同一ToolPending request与声明call id关联，不能拿并发开始顺序代替原output_index。
    let mut starts = Vec::new();
    for (position, event) in events.iter().enumerate() {
        if let AgentEvent::ToolCall { call_id, .. } = event {
            let request = events
                .iter()
                .take(position)
                .rev()
                .find_map(|event| match event {
                    AgentEvent::ExecutionStatus(status)
                        if status.status == echo_core::agent::ExecutionStatusKind::ToolPending =>
                    {
                        status.request_id.as_deref()
                    }
                    _ => None,
                });
            assert_eq!(request, Some("response-multi"));
            starts.push(call_id.clone());
        }
    }
    starts.sort();
    assert_eq!(
        starts,
        vec!["fast-call".to_string(), "slow-call".to_string()]
    );
    assert_eq!(answers(&events), vec!["两项都已完成"]);
    drop(agent);
    drop(conversations);
    let reopened = crate::memory::FileConversationStore::new(dir.path())?;
    let restored = echo_state::memory::conversation::restore_messages(
        &reopened.get_messages("native-source-order").await?,
    )?;
    let archived = restored
        .iter()
        .find(|message| {
            message
                .tool_calls
                .as_ref()
                .is_some_and(|calls| calls.iter().any(|call| call.id == "slow-call"))
        })
        .ok_or_else(|| ReactError::Other("cold tool declaration missing".into()))?;
    assert_eq!(
        archived.assistant_blocks.as_deref(),
        Some(expected.as_slice())
    );
    assert_eq!(
        archived.tool_calls.as_ref().map(|calls| calls
            .iter()
            .filter_map(|call| call.provider_source.clone())
            .collect::<Vec<_>>()),
        Some(sources)
    );
    Ok(())
}

#[tokio::test]
async fn native_message_unknown_normal_stop_is_candidate_without_rewriting_provider_phase()
-> Result<()> {
    let unknown = block(
        "unphased-message",
        "response-unknown",
        0,
        AssistantPhase::Unknown,
        "正常无阶段回答",
    );
    let llm = Arc::new(MockLlmClient::new().with_stream_script(vec![
        native_delta(vec![unknown.clone()], Value::Null)?,
        terminal("stop"),
    ]));
    let events = collect(&mock_agent(llm)?).await?;
    assert_eq!(answers(&events), vec!["正常无阶段回答"]);
    assert!(
        blocks(&events).contains(&&unknown),
        "provider Unknown 不是 runtime FinalAnswer phase"
    );
    assert_one_settled_execution(&events)
}

async fn failed_stream(chunks: Vec<StreamChunk>) -> Result<()> {
    let llm = Arc::new(MockLlmClient::new().with_stream_script(chunks));
    let raw = collect_raw(&mock_agent(llm)?, StreamMode::Chat).await?;
    let has_error = raw
        .iter()
        .any(|e| e.is_err() || matches!(e, Ok(AgentEvent::Error { .. })));
    let events = raw
        .into_iter()
        .filter_map(std::result::Result::ok)
        .collect::<Vec<_>>();
    assert!(has_error, "非正常响应须有明确失败事实：{events:?}");
    assert!(answers(&events).is_empty(), "截断/断流/空不能发布成功");
    assert!(
        !status_values(&events)?
            .iter()
            .any(|e| e.pointer("/data/status").and_then(Value::as_str) == Some("settled"))
    );
    Ok(())
}
#[tokio::test]
async fn native_message_length_does_not_succeed() -> Result<()> {
    failed_stream(vec![
        native_delta(
            vec![block(
                "cut",
                "r-cut",
                0,
                AssistantPhase::FinalAnswer,
                "截断候选",
            )],
            Value::Null,
        )?,
        terminal("length"),
    ])
    .await
}
#[tokio::test]
async fn native_message_abnormal_eof_does_not_succeed() -> Result<()> {
    failed_stream(vec![native_delta(
        vec![block(
            "eof",
            "r-eof",
            0,
            AssistantPhase::FinalAnswer,
            "断流候选",
        )],
        Value::Null,
    )?])
    .await
}
#[tokio::test]
async fn native_message_midstream_error_does_not_succeed() -> Result<()> {
    failed_stream(vec![
        native_delta(
            vec![block(
                "error",
                "r-error",
                0,
                AssistantPhase::FinalAnswer,
                "异常候选",
            )],
            Value::Null,
        )?,
        StreamChunk::Err(ReactError::Other("fixture transport disconnected".into())),
    ])
    .await
}
#[tokio::test]
async fn native_message_empty_normal_stop_does_not_succeed() -> Result<()> {
    failed_stream(vec![terminal("stop")]).await
}
#[tokio::test]
async fn native_message_reasoning_only_is_not_final() -> Result<()> {
    let mut reasoning = block(
        "thinking-only",
        "r-thinking",
        0,
        AssistantPhase::Reasoning,
        "",
    );
    reasoning.reasoning = Some(ReasoningBlock::Signed {
        thinking: "内部推理".into(),
        signature: "sig-only".into(),
    });
    failed_stream(vec![
        native_delta(vec![reasoning], Value::Null)?,
        terminal("stop"),
    ])
    .await
}

struct BlockSecret {
    output_calls: Arc<AtomicUsize>,
}

struct InputUnknownGuard(Arc<AtomicUsize>);
impl Guard for InputUnknownGuard {
    fn name(&self) -> &str {
        "original-input-unknown"
    }
    fn check<'a>(
        &'a self,
        _: &'a str,
        direction: GuardDirection,
    ) -> BoxFuture<'a, Result<GuardResult>> {
        Box::pin(async move {
            if direction == GuardDirection::Input {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(ReactError::ExecutionNotSettled {
                    run_id: "guard-original-run".into(),
                    execution_id: Some("guard-original-execution".into()),
                })
            } else {
                Ok(GuardResult::Pass)
            }
        })
    }
}

// 只接收当前 Driver 的有限结果；不建立第二执行器或补造 source 终态。
struct NativeContractFiniteSink;
#[async_trait::async_trait]
impl crate::runtime::EventSink for NativeContractFiniteSink {
    async fn on_event(
        &self,
        _event: echo_core::agent::EventEnvelope,
    ) -> Result<crate::runtime::SinkControl> {
        Ok(crate::runtime::SinkControl::Continue)
    }
}

#[tokio::test]
async fn native_message_input_guard_unknown_retains_original_invocation_owner() -> Result<()> {
    for streaming_return in [false, true] {
        let drops = Arc::new(AtomicUsize::new(0));
        let guard_calls = Arc::new(AtomicUsize::new(0));
        let model = Arc::new(MockLlmClient::new().with_response("must not call model"));
        let agent = ReactAgentBuilder::new()
            .llm_client(model.clone())
            .guard(Arc::new(InputUnknownGuard(guard_calls.clone())))
            .build()?;
        let runtime = echo_core::tools::ExternalRunContext {
            run_id: Some("guard-original-run".into()),
            execution_id: Some("guard-original-execution".into()),
            resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                InvocationDropProbe(drops.clone()),
            )],
            ..Default::default()
        };
        let error = if streaming_return {
            let invocation = echo_core::agent::AgentInvocationContext {
                runtime: Some(runtime),
                ..Default::default()
            };
            agent
                .execute_stream_message_with_invocation_context(
                    Message::user("guard input".into()),
                    CancellationToken::new(),
                    invocation,
                )
                .await
                .err()
                .ok_or_else(|| {
                    ReactError::Other("guard unknown unexpectedly returned a stream".into())
                })?
        } else {
            let identity = echo_core::agent::EventIdentity::for_run("guard-original-run")?
                .with_execution_id("guard-original-execution")?;
            let invocation = echo_core::agent::AgentInvocationContext {
                runtime: Some(runtime),
                ..Default::default()
            };
            crate::runtime::AgentTurnDriver
                .drive(
                    &agent,
                    crate::runtime::TurnRequest::from_message(
                        identity,
                        Message::user("guard input".into()),
                    )
                    .mode(crate::runtime::TurnMode::Execute)
                    .invocation(invocation),
                    &NativeContractFiniteSink,
                )
                .await
                .err()
                .ok_or_else(|| {
                    ReactError::Other("guard unknown unexpectedly returned a finite receipt".into())
                })?
        };
        assert!(
            matches!(error, ReactError::ExecutionNotSettled { ref run_id, execution_id: Some(ref execution) } if run_id == "guard-original-run" && execution == "guard-original-execution")
        );
        assert_eq!(
            drops.load(Ordering::SeqCst),
            0,
            "准备期unknown也必须保留原资源束"
        );
        assert!(matches!(
            agent.close().await,
            Err(ReactError::ExecutionNotSettled { .. })
        ));
        assert!(matches!(
            agent.execute("new admission").await,
            Err(ReactError::ExecutionNotSettled { .. })
        ));
        assert_eq!(guard_calls.load(Ordering::SeqCst), 1);
        assert_eq!(model.call_count(), 0);
    }
    Ok(())
}
impl Guard for BlockSecret {
    fn name(&self) -> &str {
        "native-whole-output-guard"
    }
    fn check<'a>(
        &'a self,
        content: &'a str,
        direction: GuardDirection,
    ) -> BoxFuture<'a, Result<GuardResult>> {
        Box::pin(async move {
            Ok(
                if direction == GuardDirection::Output && content.contains("SECRET-DRAFT") {
                    self.output_calls.fetch_add(1, Ordering::SeqCst);
                    GuardResult::Block {
                        reason: "whole output rejected".into(),
                    }
                } else {
                    GuardResult::Pass
                },
            )
        })
    }
}
#[tokio::test]
async fn native_message_guard_rejects_without_whole_output_leak() -> Result<()> {
    let llm = Arc::new(MockLlmClient::new().with_stream_script(vec![
        native_delta(
            vec![block(
                "guard-draft",
                "r-guard",
                0,
                AssistantPhase::FinalAnswer,
                "SECRET-DRAFT",
            )],
            Value::Null,
        )?,
        terminal("stop"),
    ]));
    let output_calls = Arc::new(AtomicUsize::new(0));
    let agent = ReactAgentBuilder::new()
        .llm_client(llm)
        .guard(Arc::new(BlockSecret {
            output_calls: output_calls.clone(),
        }))
        .build()?;
    let raw = collect_raw(&agent, StreamMode::Chat).await?;
    let events = raw
        .into_iter()
        .filter_map(std::result::Result::ok)
        .collect::<Vec<_>>();
    assert_eq!(
        output_calls.load(Ordering::SeqCst),
        1,
        "必须真实到达Output Guard"
    );
    assert!(events.iter().any(
        |e| matches!(e,AgentEvent::Error{message,..} if message.contains("whole output rejected"))
    ));
    assert!(answers(&events).is_empty());
    assert!(
        !visible_text(&events).contains("SECRET-DRAFT"),
        "全量Guard前不得经Token/native块泄露"
    );
    Ok(())
}
#[tokio::test]
async fn native_message_structured_rejection_never_publishes_invalid_candidate() -> Result<()> {
    let llm = Arc::new(
        MockLlmClient::new()
            .with_stream_script(vec![
                native_delta(
                    vec![block(
                        "bad-json",
                        "r-json-bad",
                        0,
                        AssistantPhase::FinalAnswer,
                        r#"{"age":"SECRET-DRAFT"}"#,
                    )],
                    Value::Null,
                )?,
                terminal("stop"),
            ])
            .with_response(r#"{"age":28}"#),
    );
    let agent = ReactAgentBuilder::new()
        .llm_client(llm.clone())
        .model_profile(
            echo_core::llm::capabilities::ModelProfile::from_provider_name("mock-model", "openai"),
        )
        .response_format(crate::llm::ResponseFormat::json_schema(
            "person",
            json!({"type":"object","properties":{"age":{"type":"integer"}},"required":["age"]}),
        ))
        .build()?;
    let events = collect(&agent).await?;
    assert_eq!(answers(&events), vec![r#"{"age":28}"#]);
    assert_eq!(llm.call_count(), 2);
    assert!(!visible_text(&events).contains("SECRET-DRAFT"));
    Ok(())
}
struct RejectFirstCritic {
    calls: AtomicUsize,
}
impl Critic for RejectFirstCritic {
    fn critique<'a>(
        &'a self,
        _task: &'a str,
        _answer: &'a str,
        _context: &'a str,
    ) -> BoxFuture<'a, Result<Critique>> {
        Box::pin(async move {
            let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
            Ok(Critique {
                score: if first { 0.0 } else { 10.0 },
                passed: !first,
                feedback: "retry without disclosure".into(),
                suggestions: vec![],
            })
        })
    }
}
#[tokio::test]
async fn native_message_critic_rejection_never_publishes_rejected_candidate() -> Result<()> {
    let llm = Arc::new(
        MockLlmClient::new()
            .with_response("SECRET-DRAFT")
            .with_response("已通过审阅"),
    );
    let critic = Arc::new(RejectFirstCritic {
        calls: AtomicUsize::new(0),
    });
    let mut agent = mock_agent(llm.clone())?;
    agent.config_mut().verifier_enabled = true;
    agent.set_critic(critic.clone());
    let events = collect(&agent).await?;
    assert_eq!(critic.calls.load(Ordering::SeqCst), 2);
    assert_eq!(answers(&events), vec!["已通过审阅"]);
    assert!(!visible_text(&events).contains("SECRET-DRAFT"));
    Ok(())
}

#[tokio::test]
async fn native_message_critic_retry_budget_does_not_approve_a_rejected_answer() -> Result<()> {
    let llm = Arc::new(MockLlmClient::new().with_responses([
        "REJECTED-0",
        "REJECTED-1",
        "REJECTED-2",
        "REJECTED-3",
    ]));
    let mut agent = mock_agent(llm)?;
    agent.config_mut().verifier_enabled = true;
    agent.config_mut().verifier_max_retries = 2;
    agent.set_critic(Arc::new(echo_core::agent::StaticCritic::always_fail()));
    let raw = collect_raw(&agent, StreamMode::Chat).await?;
    assert!(
        !raw.iter()
            .any(|event| matches!(event, Ok(AgentEvent::FinalAnswer(_)))),
        "重试预算耗尽不能把已拒绝候选变成成功"
    );
    assert!(
        raw.iter()
            .any(|event| event.is_err() || matches!(event, Ok(AgentEvent::Error { .. })))
    );
    Ok(())
}

struct UnavailableCritic;
impl Critic for UnavailableCritic {
    fn critique<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<Critique>> {
        Box::pin(async { Err(ReactError::Other("critic backend unavailable".to_string())) })
    }
}

#[tokio::test]
async fn native_message_critic_backend_failure_cannot_publish_success() -> Result<()> {
    let llm = Arc::new(MockLlmClient::new().with_response("UNVERIFIED-CONTENT"));
    let mut agent = mock_agent(llm)?;
    agent.config_mut().verifier_enabled = true;
    agent.set_critic(Arc::new(UnavailableCritic));
    let raw = collect_raw(&agent, StreamMode::Chat).await?;
    assert!(
        !raw.iter()
            .any(|event| matches!(event, Ok(AgentEvent::FinalAnswer(_)))),
        "已配置Critic失败不表示校验通过"
    );
    assert!(raw.iter().any(|event| match event {
        Err(error) => error.to_string().contains("critic backend unavailable"),
        Ok(AgentEvent::Error { message, .. }) => message.contains("critic backend unavailable"),
        _ => false,
    }));
    Ok(())
}
#[tokio::test]
async fn native_message_stop_continues_same_execution_and_publishes_success_once() -> Result<()> {
    let llm = Arc::new(
        MockLlmClient::new()
            .with_response("STOP-REJECTED-DRAFT")
            .with_response("Stop之后完成"),
    );
    let agent = mock_agent(llm.clone())?;
    let calls = Arc::new(AtomicUsize::new(0));
    let hook_calls = calls.clone();
    agent.hook_registry().write().await.set_programmatic_hook(
        "native-continue-once",
        &[crate::skills::hooks::HookEvent::Stop],
        Arc::new(move |_ctx| {
            let count = hook_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                let mut result = crate::skills::hooks::HookResult::allow();
                if count == 0 {
                    result.continue_reason = Some("检查未结束，继续同一执行".into());
                }
                result
            })
        }),
    );
    let events = collect(&agent).await?;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(llm.call_count(), 2);
    assert_eq!(answers(&events), vec!["Stop之后完成"]);
    assert!(!visible_text(&events).contains("STOP-REJECTED-DRAFT"));
    assert_one_settled_execution(&events)
}

fn fixture_dir() -> Result<tempfile::TempDir> {
    // 测试运行时不使用系统 TMP；所有临时持久化样本都留在用户指定主根。
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.echo-semantic/local/parallel-executors-20261027/runtime-acceptance/tmp");
    std::fs::create_dir_all(&base)?;
    Ok(tempfile::Builder::new()
        .prefix("native-message-")
        .tempdir_in(base)?)
}

#[tokio::test]
async fn native_message_cancel_after_completed_effect_then_cold_resume_does_not_replay()
-> Result<()> {
    use crate::state::RuntimeStateStore;
    let dir = fixture_dir()?;
    let effects = Arc::new(AtomicUsize::new(0));
    let llm = Arc::new(
        MockLlmClient::new()
            .then_tool_call("durable-effect", "effect", "{}")
            .with_stream_script(vec![
                StreamChunk::Delay(Duration::from_secs(30)),
                terminal("stop"),
            ]),
    );
    let runtime = Arc::new(crate::state::FileRuntimeStateStore::new(dir.path())?);
    let agent = ReactAgentBuilder::new()
        .llm_client(llm)
        .conversation_id("native-resume")
        .state_store(runtime.clone())
        .tool(Box::new(EffectTool {
            name: "effect",
            effects: effects.clone(),
            delay: Duration::ZERO,
        }))
        .build()?;
    let cancel = CancellationToken::new();
    let mut stream = agent
        .execute_stream_with_cancel("effect then finish", cancel.clone())
        .await?;
    let mut observed = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.next().await {
            let event = event?;
            let completed = matches!(event, AgentEvent::ToolBatchEnd);
            observed.push(event);
            if completed {
                return Ok(());
            }
        }
        Err(ReactError::Other(
            "completed tool batch was not observed".into(),
        ))
    })
    .await
    .map_err(|_| ReactError::Other("tool batch barrier timed out".into()))??;
    cancel.cancel();
    let tail = tokio::time::timeout(Duration::from_secs(5), stream.collect::<Vec<_>>())
        .await
        .map_err(|_| ReactError::Other("cancel did not physically settle".into()))?;
    observed.extend(tail.into_iter().collect::<Result<Vec<_>>>()?);
    assert!(answers(&observed).is_empty());
    assert!(
        observed.iter().any(|e| matches!(e, AgentEvent::Cancelled)),
        "取消原始观察：{observed:#?}"
    );
    assert!(
        matches!(observed.last(),Some(AgentEvent::ExecutionStatus(status)) if status.status==echo_core::agent::ExecutionStatusKind::Cancelled)
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let checkpoint = runtime
        .get_checkpoint("native-resume")
        .await?
        .ok_or_else(|| ReactError::Other("cancel checkpoint missing".into()))?;
    assert_eq!(
        checkpoint.completed_tool_call_ids()?,
        vec!["durable-effect"]
    );
    drop(agent);
    drop(runtime);
    let reopened = Arc::new(crate::state::FileRuntimeStateStore::new(dir.path())?);
    let resumed_llm = Arc::new(MockLlmClient::new().with_response("恢复完成"));
    let resumed = ReactAgentBuilder::new()
        .llm_client(resumed_llm.clone())
        .conversation_id("native-resume")
        .state_store(reopened)
        .tool(Box::new(EffectTool {
            name: "effect",
            effects: effects.clone(),
            delay: Duration::ZERO,
        }))
        .build()?;
    let events = collect_raw(&resumed, StreamMode::Execute)
        .await?
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(answers(&events), vec!["恢复完成"]);
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "冷恢复不得重放已完成副作用"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e,AgentEvent::ToolCall{call_id,..} if call_id=="durable-effect"))
    );
    let calls = resumed_llm.all_calls();
    assert_eq!(
        calls
            .first()
            .into_iter()
            .flatten()
            .filter(|m| m.tool_call_id.as_deref() == Some("durable-effect"))
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn native_message_file_conversation_reload_preserves_typed_blocks_and_native_reasoning()
-> Result<()> {
    let dir = fixture_dir()?;
    let signed = ReasoningBlock::Signed {
        thinking: "签名原文 🧪".into(),
        signature: "signature-untouched==".into(),
    };
    let opaque = ReasoningBlock::Opaque {
        provider: "openai_responses".into(),
        id: "rs_original".into(),
        data: "encrypted-original==".into(),
        summary: vec!["原摘要".into()],
    };
    let mut signed_block = block(
        "native-signed",
        "r-persist",
        0,
        AssistantPhase::Reasoning,
        "",
    );
    signed_block.reasoning = Some(signed.clone());
    let mut opaque_block = block("rs_original", "r-persist", 1, AssistantPhase::Reasoning, "");
    opaque_block.reasoning = Some(opaque.clone());
    let progress = block(
        "progress-persist",
        "r-persist",
        2,
        AssistantPhase::Commentary,
        "工具前公开正文",
    );
    let expected = vec![signed_block, opaque_block, progress];
    let llm = Arc::new(
        MockLlmClient::new()
            .with_stream_script(vec![
                native_delta(expected.clone(), json!([call(0, "read-once", "effect")]))?,
                terminal("tool_calls"),
            ])
            .with_response("持久化后最终回答"),
    );
    let conversations = Arc::new(crate::memory::FileConversationStore::new(dir.path())?);
    let runtime = Arc::new(crate::state::FileRuntimeStateStore::new(dir.path())?);
    let agent = ReactAgentBuilder::new()
        .llm_client(llm.clone())
        .conversation_id("native-history")
        .conversation_store(conversations.clone())
        .state_store(runtime.clone())
        .tool(Box::new(EffectTool {
            name: "effect",
            effects: Arc::new(AtomicUsize::new(0)),
            delay: Duration::ZERO,
        }))
        .build()?;
    let events = collect(&agent).await?;
    assert_eq!(answers(&events), vec!["持久化后最终回答"]);
    let model_calls = llm.all_calls();
    let replay = model_calls
        .get(1)
        .ok_or_else(|| ReactError::Other("missing replay request".into()))?;
    assert!(
        replay
            .iter()
            .any(|m| m.reasoning_blocks.as_deref() == Some(&[signed.clone(), opaque.clone()]))
    );
    drop(agent);
    drop(conversations);
    drop(runtime);
    let reopened = crate::memory::FileConversationStore::new(dir.path())?;
    let stored = reopened.get_messages("native-history").await?;
    let restored = echo_state::memory::conversation::restore_messages(&stored)?;
    let restored_blocks: Vec<_> = restored
        .iter()
        .flat_map(|m| m.assistant_blocks.iter().flatten())
        .cloned()
        .collect();
    for original in &expected {
        assert!(
            restored_blocks.contains(original),
            "真实ConversationStore丢native块：{original:?}"
        );
    }
    assert!(
        restored
            .iter()
            .any(|m| m.reasoning_blocks.as_deref() == Some(&[signed.clone(), opaque.clone()]))
    );
    assert!(
        !restored_blocks
            .iter()
            .any(|b| b.phase == AssistantPhase::Reasoning && b.delta.contains("工具前公开正文"))
    );
    // phase=final 的模型块不等同执行确认：只有上面的运行事件确认成功；重载本身不补发 FinalAnswer。
    assert_eq!(answers(&events).len(), 1);
    Ok(())
}

#[tokio::test]
async fn native_message_incremental_block_keeps_identity_and_is_not_a_second_answer() -> Result<()>
{
    let mut first = block(
        "incremental-message",
        "response-incremental",
        0,
        AssistantPhase::FinalAnswer,
        "完整",
    );
    first.complete = false;
    let last = block(
        "incremental-message",
        "response-incremental",
        1,
        AssistantPhase::FinalAnswer,
        "回答 🦉",
    );
    let llm = Arc::new(MockLlmClient::new().with_stream_script(vec![
        native_delta(vec![first.clone()], Value::Null)?,
        native_delta(vec![last.clone()], Value::Null)?,
        terminal("stop"),
    ]));
    let events = collect(&mock_agent(llm)?).await?;
    let observed: Vec<_> = blocks(&events)
        .into_iter()
        .filter(|b| b.request_id == "response-incremental")
        .cloned()
        .collect();
    assert_eq!(
        observed,
        vec![first, last],
        "增量不丢 provider item/request 身份和 complete 事实"
    );
    assert_eq!(answers(&events), vec!["完整回答 🦉"]);
    assert_one_settled_execution(&events)
}

#[tokio::test]
async fn native_message_reloaded_model_final_is_not_runtime_confirmed_final() -> Result<()> {
    let dir = fixture_dir()?;
    let draft = block(
        "model-final-before-stop",
        "response-before-stop",
        0,
        AssistantPhase::FinalAnswer,
        "模型声明完成但Stop要求继续",
    );
    let llm = Arc::new(
        MockLlmClient::new()
            .with_stream_script(vec![
                native_delta(vec![draft.clone()], Value::Null)?,
                terminal("stop"),
            ])
            .with_response("运行时最终确认"),
    );
    let conversations = Arc::new(crate::memory::FileConversationStore::new(dir.path())?);
    use crate::trace::RunStore;
    let traces = Arc::new(crate::trace::JsonlRunStore::new(dir.path().join("runs"))?);
    let agent = ReactAgentBuilder::new()
        .llm_client(llm)
        .with_run_store(traces.clone())
        .conversation_id("native-final-distinction")
        .conversation_store(conversations.clone())
        .state_store(Arc::new(crate::state::FileRuntimeStateStore::new(
            dir.path(),
        )?))
        .build()?;
    let calls = Arc::new(AtomicUsize::new(0));
    agent.hook_registry().write().await.set_programmatic_hook(
        "continue-model-final",
        &[crate::skills::hooks::HookEvent::Stop],
        Arc::new(move |_ctx| {
            let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
            Box::pin(async move {
                let mut result = crate::skills::hooks::HookResult::allow();
                if first {
                    result.continue_reason = Some("还有真实验收未完成".into());
                }
                result
            })
        }),
    );
    let events = collect(&agent).await?;
    assert_eq!(
        answers(&events),
        vec!["运行时最终确认"],
        "模型块完成不发布Agent执行完成"
    );
    assert!(!visible_text(&events).contains("模型声明完成但Stop要求继续"));
    drop(agent);
    drop(conversations);
    drop(traces);
    let reopened_traces = crate::trace::JsonlRunStore::new(dir.path().join("runs"))?;
    let summaries = reopened_traces.list_all(10).await?;
    assert_eq!(summaries.len(), 1, "Stop继续不另建Agent执行");
    let summary = summaries
        .first()
        .ok_or_else(|| ReactError::Other("missing durable execution".into()))?;
    let completed = reopened_traces
        .load(&summary.run_id)
        .await?
        .ok_or_else(|| ReactError::Other("missing durable completion receipt".into()))?;
    assert_eq!(completed.status, crate::trace::RunStatus::Completed);
    assert_eq!(completed.final_output.as_deref(), Some("运行时最终确认"));
    assert!(completed.finished_at.is_some());
    let reopened = crate::memory::FileConversationStore::new(dir.path())?;
    let restored = echo_state::memory::conversation::restore_messages(
        &reopened.get_messages("native-final-distinction").await?,
    )?;
    let model_only = restored
        .iter()
        .find(|m| {
            m.assistant_blocks
                .as_ref()
                .is_some_and(|blocks| blocks.contains(&draft))
        })
        .ok_or_else(|| ReactError::Other("model final not restored".into()))?;
    assert!(
        model_only.execution_status.as_ref().is_some_and(|status|status.status==echo_core::agent::ExecutionStatusKind::Candidate),
        "模型final/Stop继续只保Candidate坐标，不能伪造耐久运行确认"
    );
    assert!(
        model_only
            .approved_assistant_blocks
            .as_ref()
            .is_some_and(Vec::is_empty),
        "Stop继续候选raw Unknown/final均无呈现许可"
    );
    let runtime_final = restored
        .iter()
        .find(|m| m.content.as_text().as_deref() == Some("运行时最终确认"))
        .ok_or_else(|| ReactError::Other("confirmed final message not restored".into()))?;
    let confirmation = runtime_final
        .execution_status
        .as_ref()
        .ok_or_else(|| ReactError::Other("durable runtime confirmation lost".into()))?;
    assert_eq!(
        confirmation.status,
        echo_core::agent::ExecutionStatusKind::Candidate
    );
    let published = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ExecutionStatus(status)
                if status.status == echo_core::agent::ExecutionStatusKind::Settled =>
            {
                Some(status)
            }
            _ => None,
        })
        .ok_or_else(|| ReactError::Other("public completion receipt missing".into()))?;
    assert_eq!(confirmation.execution_id, published.execution_id);
    assert_eq!(confirmation.request_id, published.request_id);
    assert_eq!(confirmation.sequence, published.sequence);
    // 只有物理结算后实际公开的status可被原ChatEventLog记为确认；原Message中的Candidate不是成功。
    assert_ne!(confirmation.status, published.status);
    assert!(
        restored
            .iter()
            .flat_map(|m| m.assistant_blocks.iter().flatten())
            .any(|b| b == &draft),
        "模型阶段原件仍应保留供回放，不能伪改为运行时终态"
    );
    assert_one_settled_execution(&events)
}

async fn same_response_progress_and_final(phase: AssistantPhase) -> Result<()> {
    let progress = block(
        "progress-same-response",
        "r-mixed-final",
        0,
        phase,
        "过程进度",
    );
    let final_block = block(
        "answer-same-response",
        "r-mixed-final",
        1,
        AssistantPhase::FinalAnswer,
        "真正最终正文",
    );
    let llm = Arc::new(MockLlmClient::new().with_stream_script(vec![
        native_delta(vec![progress.clone(), final_block.clone()], Value::Null)?,
        terminal("stop"),
    ]));
    let events = collect(&mock_agent(llm.clone())?).await?;
    assert_eq!(llm.call_count(), 1);
    assert_eq!(answers(&events), vec!["真正最终正文"], "进度不拼进最终答案");
    let native: Vec<_> = blocks(&events)
        .into_iter()
        .filter(|b| b.request_id == "r-mixed-final")
        .cloned()
        .collect();
    assert_eq!(native, vec![progress, final_block]);
    assert_one_settled_execution(&events)
}
#[tokio::test]
async fn native_message_same_response_commentary_and_final_stay_separate() -> Result<()> {
    same_response_progress_and_final(AssistantPhase::Commentary).await
}
#[tokio::test]
async fn native_message_same_response_partial_and_final_stay_separate() -> Result<()> {
    same_response_progress_and_final(AssistantPhase::PartialAnswer).await
}

struct InvocationDropProbe(Arc<AtomicUsize>);
impl Drop for InvocationDropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn native_message_public_invocation_withholds_success_until_original_session_end_owner_settles()
-> Result<()> {
    let effects = Arc::new(AtomicUsize::new(0));
    let llm = Arc::new(
        MockLlmClient::new()
            .then_tool_call("real-effect", "effect", "{}")
            .with_response("cleanup之后的确认回答"),
    );
    let agent = Arc::new(
        ReactAgentBuilder::new()
            .llm_client(llm)
            .tool(Box::new(EffectTool {
                name: "effect",
                effects: effects.clone(),
                delay: Duration::ZERO,
            }))
            .build()?,
    );
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hook_entered = entered.clone();
    let hook_release = release.clone();
    let hook_closed = closed.clone();
    agent.hook_registry().write().await.set_programmatic_hook(
        "physical-session-end",
        &[crate::skills::hooks::HookEvent::SessionEnd],
        Arc::new(move |_ctx| {
            let entered = hook_entered.clone();
            let release = hook_release.clone();
            let closed = hook_closed.clone();
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                closed.store(true, Ordering::SeqCst);
                crate::skills::hooks::HookResult::allow()
            })
        }),
    );
    let early = Arc::new(AtomicUsize::new(0));
    let guard_drops = Arc::new(AtomicUsize::new(0));
    let reader_agent = agent.clone();
    let reader_early = early.clone();
    let reader_closed = closed.clone();
    let invocation_drops = guard_drops.clone();
    let reader = tokio::spawn(async move {
        // 使用真实pub trait invocation，不直调内部run_stream_channel。
        let mut source = reader_agent
            .execute_stream_with_invocation_context(
                "do effect, finish",
                CancellationToken::new(),
                echo_core::agent::AgentInvocationContext {
                    runtime: Some(echo_core::tools::ExternalRunContext {
                        run_id: Some("public-native-run".into()),
                        execution_id: Some("public-native-execution".into()),
                        ..Default::default()
                    }),
                    resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                        InvocationDropProbe(invocation_drops),
                    )],
                    ..Default::default()
                },
            )
            .await?;
        let mut events = Vec::new();
        while let Some(event) = source.next().await {
            let event = event?;
            let success = matches!(&event, AgentEvent::FinalAnswer(_))
                || matches!(&event,AgentEvent::ExecutionStatus(status) if status.status==echo_core::agent::ExecutionStatusKind::Settled);
            if success && !reader_closed.load(Ordering::SeqCst) {
                reader_early.fetch_add(1, Ordering::SeqCst);
            }
            events.push(event);
        }
        Ok::<_, ReactError>(events)
    });
    let entered_result = tokio::time::timeout(Duration::from_secs(3), entered.notified()).await;
    let held = agent.execution_mutex.try_lock().is_err();
    let drops_before = guard_drops.load(Ordering::SeqCst);
    let early_before = early.load(Ordering::SeqCst);
    // 即使断言将失败，也先释放原hook、等待reader退出，避免测试本身遗留活owner。
    release.notify_one();
    let events = tokio::time::timeout(Duration::from_secs(3), reader)
        .await
        .map_err(|_| ReactError::Other("public cleanup did not settle".into()))?
        .map_err(|e| ReactError::Other(e.to_string()))??;
    assert!(entered_result.is_ok(), "没有进入真实SessionEnd");
    assert!(held, "原cleanup未结算时仍由原execution slot持有");
    assert_eq!(drops_before, 0, "不能提前释放原invocation resources");
    assert_eq!(early_before, 0);
    assert_eq!(
        early.load(Ordering::SeqCst),
        0,
        "cleanup期间不得发布最终成功"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(answers(&events), vec!["cleanup之后的确认回答"]);
    assert_eq!(guard_drops.load(Ordering::SeqCst), 1);
    assert!(agent.execution_mutex.try_lock().is_ok());
    assert_one_settled_execution(&events)
}

#[tokio::test]
async fn native_message_public_structured_invocation_entries_share_one_terminal_contract()
-> Result<()> {
    for chat in [true, false] {
        let llm = Arc::new(MockLlmClient::new().with_response("structured public final"));
        let agent = mock_agent(llm.clone())?;
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime: Some(echo_core::tools::ExternalRunContext {
                run_id: Some("structured-public-run".into()),
                execution_id: Some("structured-public-execution".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let stream = if chat {
            agent
                .chat_stream_message_with_invocation_context(
                    Message::user("chat".into()),
                    CancellationToken::new(),
                    invocation,
                )
                .await?
        } else {
            agent
                .execute_stream_message_with_invocation_context(
                    Message::user("execute".into()),
                    CancellationToken::new(),
                    invocation,
                )
                .await?
        };
        let events = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(answers(&events), vec!["structured public final"]);
        assert_eq!(llm.call_count(), 1);
        assert_one_settled_execution(&events)?;
    }
    Ok(())
}

struct FinalCallbackProbe {
    calls: Arc<AtomicUsize>,
    entered: Option<Arc<tokio::sync::Notify>>,
    release: Option<Arc<tokio::sync::Notify>>,
}
impl crate::agent::AgentCallback for FinalCallbackProbe {
    fn on_final_answer<'a>(&'a self, _agent: &'a str, _answer: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Some(entered) = &self.entered {
                entered.notify_one();
            }
            if let Some(release) = &self.release {
                release.notified().await;
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
        })
    }
}
#[tokio::test]
async fn native_message_public_cancel_during_session_end_does_not_publish_success_callback()
-> Result<()> {
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = ReactAgentBuilder::new()
        .llm_client(Arc::new(MockLlmClient::new().with_response("尚未确认候选")))
        .callback(Arc::new(FinalCallbackProbe {
            calls: calls.clone(),
            entered: None,
            release: None,
        }))
        .build()?;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let hook_entered = entered.clone();
    let hook_release = release.clone();
    agent.hook_registry().write().await.set_programmatic_hook(
        "cancel-final-cleanup",
        &[crate::skills::hooks::HookEvent::SessionEnd],
        Arc::new(move |_ctx| {
            let entered = hook_entered.clone();
            let release = hook_release.clone();
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                crate::skills::hooks::HookResult::allow()
            })
        }),
    );
    let cancel = CancellationToken::new();
    let source = agent
        .execute_stream_with_invocation_context("run", cancel.clone(), Default::default())
        .await?;
    let entered_result = tokio::time::timeout(Duration::from_secs(3), entered.notified()).await;
    cancel.cancel();
    release.notify_one();
    let events = tokio::time::timeout(Duration::from_secs(3), source.collect::<Vec<_>>())
        .await
        .map_err(|_| ReactError::Other("cancelled cleanup did not settle".into()))?
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    assert!(entered_result.is_ok());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "SessionEnd期间取消不能先泄露on_final_answer成功回调"
    );
    assert!(answers(&events).is_empty());
    assert!(
        matches!(events.last(),Some(AgentEvent::ExecutionStatus(status)) if status.status==echo_core::agent::ExecutionStatusKind::Cancelled)
    );
    assert!(
        !status_values(&events)?
            .iter()
            .any(|value| value.pointer("/data/status").and_then(Value::as_str) == Some("settled"))
    );
    Ok(())
}

#[test]
fn native_message_original_producer_loss_does_not_leave_durable_settled_confirmation() -> Result<()>
{
    let dir = fixture_dir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let conversations = Arc::new(crate::memory::FileConversationStore::new(dir.path())?);
    let agent = ReactAgentBuilder::new()
        .llm_client(Arc::new(
            MockLlmClient::new().with_response("未取得物理结算的候选"),
        ))
        .conversation_id("lost-native-producer")
        .conversation_store(conversations.clone())
        .state_store(Arc::new(crate::state::FileRuntimeStateStore::new(
            dir.path(),
        )?))
        .callback(Arc::new(FinalCallbackProbe {
            calls: Arc::new(AtomicUsize::new(0)),
            entered: Some(entered.clone()),
            release: Some(release),
        }))
        .build()?;
    runtime.block_on(async {
        let source = agent
            .execute_stream_with_invocation_context(
                "lose producer after persistence",
                CancellationToken::new(),
                echo_core::agent::AgentInvocationContext {
                    runtime: Some(echo_core::tools::ExternalRunContext {
                        run_id: Some("lost-original-run".into()),
                        execution_id: Some("lost-original-execution".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        let entry = tokio::time::timeout(Duration::from_secs(3), entered.notified()).await;
        drop(source);
        entry.map_err(|_| {
            ReactError::Other("candidate was not persisted before producer-loss barrier".into())
        })?;
        Ok::<_, ReactError>(())
    })?;
    // 复用原Runtime真实销毁来丢失原producer，不造第二effect owner或直接伪造成功事件。
    drop(runtime);
    drop(conversations);
    let recovery = tokio::runtime::Runtime::new()?;
    recovery.block_on(async {
        let reopened=crate::memory::FileConversationStore::new(dir.path())?;
        let restored=echo_state::memory::conversation::restore_messages(&reopened.get_messages("lost-native-producer").await?)?;
        let candidate=restored.iter().find(|message|message.content.as_text().as_deref()==Some("未取得物理结算的候选"))
            .ok_or_else(||ReactError::Other("cold reload did not reach persisted candidate".into()))?;
        assert!(candidate.execution_status.as_ref().is_some_and(|status|status.status==echo_core::agent::ExecutionStatusKind::Candidate));
        assert!(!restored.iter().any(|message|message.execution_status.as_ref().is_some_and(|status|status.status==echo_core::agent::ExecutionStatusKind::Settled)));
        let failure=agent.close().await.err().ok_or_else(||ReactError::Other("original unknown owner was silently cleared".into()))?;
        assert!(matches!(failure,ReactError::ExecutionNotSettled{run_id,execution_id} if run_id=="lost-original-run"&&execution_id.as_deref()==Some("lost-original-execution")));
        Ok::<_,ReactError>(())
    })
}

struct ThoughtProbe(Arc<std::sync::Mutex<Vec<String>>>);
impl crate::agent::AgentCallback for ThoughtProbe {
    fn on_think_end<'a>(
        &'a self,
        _agent: &'a str,
        steps: &'a [echo_core::agent::StepType],
        _pt: usize,
        _ct: usize,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut seen = self.0.lock().unwrap_or_else(|error| error.into_inner());
            for step in steps {
                if let echo_core::agent::StepType::Thought(text) = step {
                    seen.push(text.clone());
                }
            }
        })
    }
}
#[tokio::test]
async fn native_message_plain_and_tool_adjacent_text_never_become_reasoning_or_duplicate_tokens()
-> Result<()> {
    for mixed in [false, true] {
        let thoughts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let llm = if mixed {
            MockLlmClient::new()
                .with_stream_script(vec![
                    native_delta(
                        vec![block(
                            "ordinary-tool-prose",
                            "r-prose",
                            0,
                            AssistantPhase::Unknown,
                            "普通工具前正文",
                        )],
                        json!([call(0, "prose-effect", "effect")]),
                    )?,
                    terminal("tool_calls"),
                ])
                .with_response("普通最终正文")
        } else {
            MockLlmClient::new().with_response("普通最终正文")
        };
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .callback(Arc::new(ThoughtProbe(thoughts.clone())))
            .tool(Box::new(EffectTool {
                name: "effect",
                effects: Arc::new(AtomicUsize::new(0)),
                delay: Duration::ZERO,
            }))
            .build()?;
        let events = collect(&agent).await?;
        assert!(
            thoughts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty(),
            "普通答复不作为on_think_end Thought"
        );
        assert!(
            !blocks(&events)
                .iter()
                .any(|block| block.phase == AssistantPhase::Reasoning)
        );
        assert_eq!(
            blocks(&events)
                .iter()
                .filter(|block| block.delta == "普通最终正文")
                .count(),
            1,
            "正文只有一个canonical块"
        );
        if mixed {
            assert_eq!(
                blocks(&events)
                    .iter()
                    .filter(|block| block.delta == "普通工具前正文")
                    .count(),
                1
            );
        }
    }
    Ok(())
}
#[tokio::test]
async fn native_message_only_native_reasoning_or_summary_reaches_thinking_callback() -> Result<()> {
    let thoughts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut reasoning = block(
        "real-native-thinking",
        "r-thinking-callback",
        0,
        AssistantPhase::Reasoning,
        "",
    );
    reasoning.reasoning = Some(ReasoningBlock::Signed {
        thinking: "原生思考原文".into(),
        signature: "sig-preserved".into(),
    });
    let llm = MockLlmClient::new().with_stream_script(vec![
        native_delta(
            vec![
                reasoning,
                block(
                    "ordinary-answer",
                    "r-thinking-callback",
                    1,
                    AssistantPhase::FinalAnswer,
                    "普通回答",
                ),
            ],
            Value::Null,
        )?,
        terminal("stop"),
    ]);
    let agent = ReactAgentBuilder::new()
        .llm_client(Arc::new(llm))
        .callback(Arc::new(ThoughtProbe(thoughts.clone())))
        .build()?;
    let events = collect(&agent).await?;
    assert_eq!(
        *thoughts.lock().unwrap_or_else(|error| error.into_inner()),
        vec!["原生思考原文"]
    );
    assert_eq!(answers(&events), vec!["普通回答"]);
    Ok(())
}

struct TransformUnknownGuard(Arc<AtomicUsize>);
impl Guard for TransformUnknownGuard {
    fn name(&self) -> &str {
        "transform-raw-unknown"
    }
    fn check<'a>(
        &'a self,
        content: &'a str,
        direction: GuardDirection,
    ) -> BoxFuture<'a, Result<GuardResult>> {
        Box::pin(async move {
            if direction == GuardDirection::Output && content == "SECRET-DRAFT" {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(GuardResult::Transform {
                    content: "已校验的安全正文".into(),
                    reasons: vec!["redacted fixture".into()],
                })
            } else {
                Ok(GuardResult::Pass)
            }
        })
    }
}
#[tokio::test]
async fn native_message_guard_transform_unknown_multi_blocks_cold_reload_has_explicit_safe_projection()
-> Result<()> {
    use sha2::Digest;
    for transform in [true, false] {
        let dir = fixture_dir()?;
        let raw = vec![
            block(
                "unknown-a",
                "r-guard-restore",
                0,
                AssistantPhase::Unknown,
                "SECRET-",
            ),
            block(
                "unknown-b",
                "r-guard-restore",
                1,
                AssistantPhase::Unknown,
                "DRAFT",
            ),
        ];
        let llm = Arc::new(MockLlmClient::new().with_stream_script(vec![
            native_delta(raw.clone(), Value::Null)?,
            terminal("stop"),
        ]));
        let output_calls = Arc::new(AtomicUsize::new(0));
        let conversations = Arc::new(crate::memory::FileConversationStore::new(dir.path())?);
        let mut builder = ReactAgentBuilder::new()
            .llm_client(llm)
            .conversation_id("guard-unknown-restore")
            .conversation_store(conversations.clone())
            .state_store(Arc::new(crate::state::FileRuntimeStateStore::new(
                dir.path(),
            )?));
        if transform {
            builder = builder.guard(Arc::new(TransformUnknownGuard(output_calls.clone())));
        }
        let agent = builder.build()?;
        let events = collect(&agent).await?;
        let expected = if transform {
            "已校验的安全正文"
        } else {
            "SECRET-DRAFT"
        };
        assert_eq!(answers(&events), vec![expected]);
        if transform {
            assert_eq!(
                output_calls.load(Ordering::SeqCst),
                1,
                "真实Output Guard转换必须实际执行"
            );
            assert!(!visible_text(&events).contains("SECRET-"));
            assert!(!visible_text(&events).contains("DRAFT"));
        }
        assert_one_settled_execution(&events)?;
        drop(agent);
        drop(conversations);
        let reopened = crate::memory::FileConversationStore::new(dir.path())?;
        let restored = echo_state::memory::conversation::restore_messages(
            &reopened.get_messages("guard-unknown-restore").await?,
        )?;
        let message = restored
            .iter()
            .find(|message| message.content.as_text().as_deref() == Some(expected))
            .ok_or_else(|| ReactError::Other("safe final content was not restored".into()))?;
        assert_eq!(
            message.assistant_blocks.as_deref(),
            Some(raw.as_slice()),
            "raw provider身份/Unknown原件仍供回放"
        );
        let approved = message
            .approved_assistant_blocks
            .as_ref()
            .ok_or_else(|| ReactError::Other("explicit approval projection was lost".into()))?;
        if transform {
            assert!(approved.is_empty(), "Some(empty)禁止消费者回退raw Unknown");
        } else {
            assert_eq!(approved, &raw, "正常未转换正控不能全隐藏native正文");
        }
        let candidate = message
            .execution_status
            .as_ref()
            .ok_or_else(|| ReactError::Other("execution linkage missing".into()))?;
        assert_eq!(
            candidate.status,
            echo_core::agent::ExecutionStatusKind::Candidate,
            "raw候选绝不带Settled许可"
        );
        let published_status = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ExecutionStatus(status)
                    if status.status == echo_core::agent::ExecutionStatusKind::Settled =>
                {
                    Some(status)
                }
                _ => None,
            })
            .ok_or_else(|| ReactError::Other("actual publication status missing".into()))?;
        assert_eq!(candidate.execution_id, published_status.execution_id);
        assert_eq!(candidate.request_id, published_status.request_id);
        assert_eq!(candidate.sequence, published_status.sequence);
        let restored_text = message.content.as_text().unwrap_or_default();
        let published = answers(&events)
            .into_iter()
            .next()
            .ok_or_else(|| ReactError::Other("actual final publication missing".into()))?;
        assert_eq!(
            sha2::Sha256::digest(restored_text.as_bytes()),
            sha2::Sha256::digest(published.as_bytes()),
            "EKO可在原journal以完整内容hash校验相同正文，不猜phase"
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_message_legacy_provider_reasoning_content_uses_typed_blocks_with_real_block_end()
-> Result<()> {
    let reasoning: DeltaMessage = serde_json::from_value(
        json!({"role":"assistant","reasoning_content":"原生reasoning_content"}),
    )?;
    let text: DeltaMessage = serde_json::from_value(json!({"content":"协议无阶段的正文"}))?;
    let thoughts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let llm = MockLlmClient::new().with_stream_script(vec![
        StreamChunk::Delta(Box::new(reasoning)),
        StreamChunk::Delta(Box::new(text)),
        terminal("stop"),
    ]);
    let agent = ReactAgentBuilder::new()
        .llm_client(Arc::new(llm))
        .callback(Arc::new(ThoughtProbe(thoughts.clone())))
        .build()?;
    let events = collect(&agent).await?;
    let native = blocks(&events);
    assert_eq!(
        native
            .iter()
            .filter(
                |block| block.phase == AssistantPhase::Unknown && block.delta == "协议无阶段的正文"
            )
            .count(),
        1
    );
    assert_eq!(
        native
            .iter()
            .filter(|block| block.phase == AssistantPhase::Reasoning
                && block.delta == "原生reasoning_content")
            .count(),
        1
    );
    assert!(
        native
            .iter()
            .any(|block| block.phase == AssistantPhase::Reasoning && block.complete)
    );
    assert_eq!(
        *thoughts.lock().unwrap_or_else(|error| error.into_inner()),
        vec!["原生reasoning_content"]
    );
    assert_eq!(answers(&events), vec!["协议无阶段的正文"]);
    assert_one_settled_execution(&events)
}

#[tokio::test]
async fn native_message_provider_pause_continue_preserves_unknown_and_continues_same_execution()
-> Result<()> {
    let paused = block(
        "paused-message",
        "r-paused",
        0,
        AssistantPhase::Unknown,
        "服务端暂停后继续的原正文",
    );
    let llm = Arc::new(
        MockLlmClient::new()
            .with_stream_script(vec![
                native_delta(vec![paused.clone()], Value::Null)?,
                terminal("continue"),
            ])
            .with_response("继续后完成"),
    );
    let agent = mock_agent(llm.clone())?;
    let events = collect(&agent).await?;
    assert_eq!(llm.call_count(), 2);
    assert_eq!(answers(&events), vec!["继续后完成"]);
    assert!(
        blocks(&events).contains(&&paused),
        "pause_turn不能按tool或措辞改phase"
    );
    assert!(
        llm.all_calls()
            .get(1)
            .into_iter()
            .flatten()
            .flat_map(|message| message.assistant_blocks.iter().flatten())
            .any(|block| block == &paused)
    );
    assert_one_settled_execution(&events)
}

fn late_phase_blocks(phase: AssistantPhase, text: &str) -> Vec<AssistantMessageBlock> {
    let mut start = block(
        "late-message:content:0",
        "r-late-phase",
        0,
        AssistantPhase::Unknown,
        text,
    );
    start.complete = false;
    start.provider_source = Some(crate::llm::types::ProviderOutputSource {
        protocol: crate::llm::LlmApiProtocol::Responses,
        request_id: "r-late-phase".into(),
        item_id: "late-message".into(),
        output_index: 0,
        content_index: Some(0),
        content_kind: Some("output_text".into()),
        phase: None,
    });
    let mut end = start.clone();
    end.sequence = 1;
    end.phase = phase;
    end.delta = String::new();
    end.complete = true;
    if let Some(source) = end.provider_source.as_mut() {
        source.phase = Some(
            match phase {
                AssistantPhase::FinalAnswer => "final_answer",
                AssistantPhase::PartialAnswer => "partial_answer",
                AssistantPhase::Commentary => "commentary",
                _ => "future_phase",
            }
            .into(),
        );
    }
    vec![start, end]
}
fn block_script(observations: &[AssistantMessageBlock]) -> Result<Vec<StreamChunk>> {
    let mut script = observations
        .iter()
        .map(|observation| native_delta(vec![observation.clone()], Value::Null))
        .collect::<Result<Vec<_>>>()?;
    script.push(terminal("stop"));
    Ok(script)
}
#[tokio::test]
async fn native_message_late_final_phase_empty_delta_keeps_same_block_body_and_cold_raw_source()
-> Result<()> {
    let text = "前面Unknown增量的完整正文 🧪";
    for incremental in [true, false] {
        let dir = fixture_dir()?;
        let observations = if incremental {
            late_phase_blocks(AssistantPhase::FinalAnswer, text)
        } else {
            let mut normalized = late_phase_blocks(AssistantPhase::FinalAnswer, text)
                .into_iter()
                .last()
                .ok_or_else(|| ReactError::Other("fixture terminal missing".into()))?;
            normalized.sequence = 0;
            normalized.delta = text.into();
            vec![normalized]
        };
        let llm = Arc::new(MockLlmClient::new().with_stream_script(block_script(&observations)?));
        let conversations = Arc::new(crate::memory::FileConversationStore::new(dir.path())?);
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .conversation_id("late-final-restore")
            .conversation_store(conversations.clone())
            .state_store(Arc::new(crate::state::FileRuntimeStateStore::new(
                dir.path(),
            )?))
            .build()?;
        let events = collect(&agent).await?;
        assert_eq!(
            answers(&events),
            vec![text],
            "只按最后metadata观察的空delta取final会丢正文"
        );
        assert_eq!(llm.call_count(), 1);
        let native: Vec<_> = blocks(&events)
            .into_iter()
            .filter(|block| block.request_id == "r-late-phase")
            .cloned()
            .collect();
        assert_eq!(
            native, observations,
            "保全部raw phase/source/complete/sequence，不改写早期Unknown原件"
        );
        assert_one_settled_execution(&events)?;
        drop(agent);
        drop(conversations);
        let reopened = crate::memory::FileConversationStore::new(dir.path())?;
        let restored = echo_state::memory::conversation::restore_messages(
            &reopened.get_messages("late-final-restore").await?,
        )?;
        let final_message = restored
            .iter()
            .find(|message| message.content.as_text().as_deref() == Some(text))
            .ok_or_else(|| ReactError::Other("complete body was not reloaded".into()))?;
        assert_eq!(
            final_message.assistant_blocks.as_deref(),
            Some(observations.as_slice())
        );
        assert_eq!(
            final_message.approved_assistant_blocks.as_deref(),
            Some(observations.as_slice()),
            "Guard未转换时，metadata补全不应误判成transform而吞canonical块"
        );
    }
    let llm = Arc::new(
        MockLlmClient::new().with_stream_script(block_script(&late_phase_blocks(
            AssistantPhase::FinalAnswer,
            text,
        ))?),
    );
    assert_eq!(
        mock_agent(llm)?
            .execute("public nonstream late metadata")
            .await?,
        text
    );
    Ok(())
}
#[tokio::test]
async fn native_message_late_partial_metadata_continues_same_execution_with_original_observations()
-> Result<()> {
    let observations =
        late_phase_blocks(AssistantPhase::PartialAnswer, "先前Unknown阶段的部分回答");
    let llm = Arc::new(
        MockLlmClient::new()
            .with_stream_script(block_script(&observations)?)
            .with_response("后续真正完成"),
    );
    let events = collect(&mock_agent(llm.clone())?).await?;
    assert_eq!(llm.call_count(), 2);
    assert_eq!(answers(&events), vec!["后续真正完成"]);
    let native: Vec<_> = blocks(&events)
        .into_iter()
        .filter(|block| block.request_id == "r-late-phase")
        .cloned()
        .collect();
    assert_eq!(native, observations);
    assert!(
        llm.all_calls()
            .get(1)
            .into_iter()
            .flatten()
            .any(|message| message.assistant_blocks.as_deref() == Some(observations.as_slice()))
    );
    assert_one_settled_execution(&events)
}
#[tokio::test]
async fn native_message_late_empty_final_does_not_borrow_body_from_other_request_or_block()
-> Result<()> {
    for wrong_request in [false, true] {
        let mut observations =
            late_phase_blocks(AssistantPhase::FinalAnswer, "别的坐标正文不得被借成final");
        if let Some(last) = observations.last_mut() {
            if wrong_request {
                last.request_id = "different-request".into();
                if let Some(source) = last.provider_source.as_mut() {
                    source.request_id = "different-request".into();
                }
            } else {
                last.block_id = "late-message:content:1".into();
                if let Some(source) = last.provider_source.as_mut() {
                    source.content_index = Some(1);
                }
            }
        }
        failed_stream(block_script(&observations)?).await?;
    }
    failed_stream(block_script(&late_phase_blocks(
        AssistantPhase::FinalAnswer,
        "",
    ))?)
    .await
}
#[tokio::test]
async fn native_message_configured_critic_veto_and_min_score_both_remain_authoritative()
-> Result<()> {
    let policies: Vec<Arc<dyn Critic>> = vec![
        Arc::new(
            echo_core::agent::CompositeCritic::new(
                echo_core::agent::CompositeStrategy::AllMustPass,
            )
            .add_critic(echo_core::agent::StaticCritic::new(10.0, true, "pass"))
            .add_critic(echo_core::agent::StaticCritic::new(
                8.0,
                false,
                "explicit veto",
            )),
        ),
        Arc::new(echo_core::agent::ThresholdCritic::new(
            echo_core::agent::StaticCritic::new(8.0, true, "below stricter threshold"),
            9.0,
        )),
        Arc::new(echo_core::agent::StaticCritic::new(
            6.0,
            true,
            "below configured minimum",
        )),
    ];
    for critic in policies {
        let mut agent = mock_agent(Arc::new(
            MockLlmClient::new().with_response("POLICY-REJECTED-CONTENT"),
        ))?;
        agent.config_mut().verifier_enabled = true;
        agent.config_mut().verifier_max_retries = 0;
        agent.config_mut().verifier_min_score = 7.0;
        agent.set_critic(critic);
        let raw = collect_raw(&agent, StreamMode::Chat).await?;
        assert!(
            raw.iter()
                .any(|event| event.is_err() || matches!(event, Ok(AgentEvent::Error { .. })))
        );
        let events: Vec<_> = raw
            .into_iter()
            .filter_map(std::result::Result::ok)
            .collect();
        assert!(
            answers(&events).is_empty(),
            "明确veto或最低分不达标不能被OR变成通过"
        );
        assert!(!visible_text(&events).contains("POLICY-REJECTED-CONTENT"));
        assert!(
            matches!(events.last(),Some(AgentEvent::ExecutionStatus(status)) if status.status==echo_core::agent::ExecutionStatusKind::Failed)
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_message_repeated_stop_continue_never_silently_becomes_success() -> Result<()> {
    let model = Arc::new(MockLlmClient::new().with_responses([
        "REJECTED-STOP-0",
        "REJECTED-STOP-1",
        "REJECTED-STOP-2",
    ]));
    let final_calls = Arc::new(AtomicUsize::new(0));
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let seen = hook_calls.clone();
    let agent = ReactAgentBuilder::new()
        .llm_client(model.clone())
        .max_iterations(3)
        .callback(Arc::new(FinalCallbackProbe {
            calls: final_calls.clone(),
            entered: None,
            release: None,
        }))
        .build()?;
    agent.hook_registry().write().await.set_programmatic_hook(
        "always-continue",
        &[crate::skills::hooks::HookEvent::Stop],
        Arc::new(move |_ctx| {
            seen.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                let mut result = crate::skills::hooks::HookResult::allow();
                result.continue_reason = Some("继续，验收尚未完成".to_string());
                result
            })
        }),
    );
    let raw = collect_raw(&agent, StreamMode::Chat).await?;
    let events = raw
        .iter()
        .filter_map(|event| event.as_ref().ok())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(hook_calls.load(Ordering::SeqCst), 3);
    assert_eq!(model.call_count(), 3);
    assert_eq!(final_calls.load(Ordering::SeqCst), 0);
    assert!(answers(&events).is_empty());
    assert!(!visible_text(&events).contains("REJECTED-STOP"));
    assert!(
        raw.iter()
            .any(|event| event.is_err() || matches!(event, Ok(AgentEvent::Error { .. })))
    );
    assert!(!events.iter().any(|event|matches!(event,AgentEvent::ExecutionStatus(status) if status.status==echo_core::agent::ExecutionStatusKind::Settled)));
    Ok(())
}

#[tokio::test]
async fn native_message_stop_block_reason_continues_same_owner_before_one_success() -> Result<()> {
    let model = Arc::new(
        MockLlmClient::new().with_responses(["BLOCKED-STOP-DRAFT", "已完成Hook要求的工作"]),
    );
    let final_calls = Arc::new(AtomicUsize::new(0));
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let seen = hook_calls.clone();
    let agent = ReactAgentBuilder::new()
        .llm_client(model.clone())
        .max_iterations(3)
        .callback(Arc::new(FinalCallbackProbe {
            calls: final_calls.clone(),
            entered: None,
            release: None,
        }))
        .build()?;
    agent.hook_registry().write().await.set_programmatic_hook(
        "block-once",
        &[crate::skills::hooks::HookEvent::Stop],
        Arc::new(move |_ctx| {
            let first = seen.fetch_add(1, Ordering::SeqCst) == 0;
            Box::pin(async move {
                if first {
                    crate::skills::hooks::HookResult::deny("仍需验证结果".to_string())
                } else {
                    crate::skills::hooks::HookResult::allow()
                }
            })
        }),
    );
    let events = collect(&agent).await?;
    assert_eq!(hook_calls.load(Ordering::SeqCst), 2);
    assert_eq!(model.call_count(), 2);
    assert_eq!(final_calls.load(Ordering::SeqCst), 1);
    assert_eq!(answers(&events), vec!["已完成Hook要求的工作"]);
    assert!(!visible_text(&events).contains("BLOCKED-STOP-DRAFT"));
    assert_one_settled_execution(&events)
}

#[tokio::test]
async fn native_message_guarded_driver_unknown_retains_original_owner_and_guards() -> Result<()> {
    let drops = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(
        MockLlmClient::new().with_error(ReactError::ExecutionNotSettled {
            run_id: "nonstream-original-run".to_string(),
            execution_id: Some("nonstream-original-execution".to_string()),
        }),
    );
    let agent = ReactAgentBuilder::new().llm_client(model).build()?;
    let runtime = echo_core::tools::ExternalRunContext {
        run_id: Some("nonstream-original-run".to_string()),
        execution_id: Some("nonstream-original-execution".to_string()),
        resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
            InvocationDropProbe(drops.clone()),
        )],
        ..Default::default()
    };
    let invocation = echo_core::agent::AgentInvocationContext {
        runtime: Some(runtime),
        ..Default::default()
    };
    let identity = echo_core::agent::EventIdentity::for_run("nonstream-original-run")?
        .with_execution_id("nonstream-original-execution")?;
    let result = crate::runtime::AgentTurnDriver
        .drive(
            &agent,
            crate::runtime::TurnRequest::from_message(
                identity,
                Message::user("等待原provider物理结算".into()),
            )
            .mode(crate::runtime::TurnMode::Execute)
            .invocation(invocation),
            &NativeContractFiniteSink,
        )
        .await;
    assert!(
        matches!(result,Err(ReactError::ExecutionNotSettled { ref run_id, execution_id:Some(ref execution_id) }) if run_id=="nonstream-original-run" && execution_id=="nonstream-original-execution")
    );
    assert_eq!(
        drops.load(Ordering::SeqCst),
        0,
        "原执行未结算时有限Driver结果不能释放guard"
    );
    let close = agent.close().await;
    assert!(
        matches!(close, Err(ReactError::ExecutionNotSettled { .. })),
        "不能把未知原执行写成Failed并提前关闭"
    );
    Ok(())
}

#[tokio::test]
async fn native_message_string_guard_unknown_preserves_original_error_and_close() -> Result<()> {
    for chat in [false, true] {
        let guard_calls = Arc::new(AtomicUsize::new(0));
        let model = Arc::new(MockLlmClient::new().with_response("must not call model"));
        let agent = ReactAgentBuilder::new()
            .llm_client(model.clone())
            .guard(Arc::new(InputUnknownGuard(guard_calls.clone())))
            .build()?;
        // 普通 String API 无 invocation guard 参数；这里直接验它自己的返回/close，
        // guarded invocation 的资源义务由相邻 typed/Driver 测试独立保留。
        let result = if chat {
            agent.chat("guard input").await
        } else {
            agent.execute("guard input").await
        };
        assert!(
            matches!(result, Err(ReactError::ExecutionNotSettled { run_id, execution_id: Some(execution) })
            if run_id == "guard-original-run" && execution == "guard-original-execution")
        );
        assert!(matches!(
            agent.close().await,
            Err(ReactError::ExecutionNotSettled { .. })
        ));
        assert!(matches!(
            agent.execute("new admission").await,
            Err(ReactError::ExecutionNotSettled { .. })
        ));
        assert_eq!(guard_calls.load(Ordering::SeqCst), 1);
        assert_eq!(model.call_count(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn native_message_string_unknown_preserves_original_error_and_close() -> Result<()> {
    for chat in [false, true] {
        let model = Arc::new(
            MockLlmClient::new().with_error(ReactError::ExecutionNotSettled {
                run_id: "nonstream-original-run".into(),
                execution_id: Some("nonstream-original-execution".into()),
            }),
        );
        let agent = ReactAgentBuilder::new().llm_client(model.clone()).build()?;
        let result = if chat {
            agent.chat("等待原provider物理结算").await
        } else {
            agent.execute("等待原provider物理结算").await
        };
        assert!(
            matches!(result, Err(ReactError::ExecutionNotSettled { run_id, execution_id: Some(execution) })
            if run_id == "nonstream-original-run" && execution == "nonstream-original-execution")
        );
        assert!(matches!(
            agent.close().await,
            Err(ReactError::ExecutionNotSettled { .. })
        ));
        assert_eq!(model.call_count(), 1);
    }
    Ok(())
}

/// 只在现MockLlm的首个真实stream EOF处加barrier；执行/Steer/持久化仍走现ReAct。
struct HoldFirstEofClient {
    inner: Arc<MockLlmClient>,
    calls: AtomicUsize,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
impl crate::llm::LlmClient for HoldFirstEofClient {
    fn model_name(&self) -> &str {
        crate::llm::LlmClient::model_name(self.inner.as_ref())
    }
    fn chat(
        &self,
        request: crate::llm::ChatRequest,
    ) -> BoxFuture<'_, Result<crate::llm::ChatResponse>> {
        crate::llm::LlmClient::chat(self.inner.as_ref(), request)
    }
    fn chat_stream(
        &self,
        request: crate::llm::ChatRequest,
    ) -> BoxFuture<'_, Result<futures::stream::BoxStream<'static, Result<crate::llm::ChatChunk>>>>
    {
        Box::pin(async move {
            let mut source =
                crate::llm::LlmClient::chat_stream(self.inner.as_ref(), request).await?;
            if self.calls.fetch_add(1, Ordering::SeqCst) > 0 {
                return Ok(source);
            }
            let entered = self.entered.clone();
            let release = self.release.clone();
            let held = async_stream::stream! {
                while let Some(chunk)=source.next().await {yield chunk;}
                entered.notify_one();release.notified().await;
            };
            Ok(Box::pin(held)
                as futures::stream::BoxStream<
                    'static,
                    Result<crate::llm::ChatChunk>,
                >)
        })
    }
}
#[tokio::test]
async fn native_message_steer_superseded_unchecked_unknown_cold_reload_has_explicit_empty_approval()
-> Result<()> {
    let dir = fixture_dir()?;
    let raw = block(
        "steer-draft",
        "r-steer-draft",
        0,
        AssistantPhase::Unknown,
        "SECRET-DRAFT",
    );
    let inner = Arc::new(
        MockLlmClient::new()
            .with_stream_script(vec![
                native_delta(vec![raw.clone()], Value::Null)?,
                terminal("stop"),
            ])
            .with_response("新输入后的安全回答"),
    );
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let guard_calls = Arc::new(AtomicUsize::new(0));
    let conversations = Arc::new(crate::memory::FileConversationStore::new(dir.path())?);
    let agent = Arc::new(
        ReactAgentBuilder::new()
            .llm_client(Arc::new(HoldFirstEofClient {
                inner: inner.clone(),
                calls: AtomicUsize::new(0),
                entered: entered.clone(),
                release: release.clone(),
            }))
            .conversation_id("steer-unchecked-restore")
            .conversation_store(conversations.clone())
            .state_store(Arc::new(crate::state::FileRuntimeStateStore::new(
                dir.path(),
            )?))
            .guard(Arc::new(BlockSecret {
                output_calls: guard_calls.clone(),
            }))
            .build()?,
    );
    let reader_agent = agent.clone();
    let reader = tokio::spawn(async move {
        let source = reader_agent
            .chat_stream_message_with_invocation_context(
                Message::user("first".into()),
                CancellationToken::new(),
                Default::default(),
            )
            .await?;
        source
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
    });
    let reached = tokio::time::timeout(Duration::from_secs(3), entered.notified()).await;
    let steer = agent.steer_input(None, Message::user("supersede before validation".into()));
    release.notify_one();
    let events = tokio::time::timeout(Duration::from_secs(5), reader)
        .await
        .map_err(|_| ReactError::Other("steer fixture did not settle".into()))?
        .map_err(|error| ReactError::Other(error.to_string()))??;
    assert!(reached.is_ok());
    steer.map_err(|error| ReactError::Other(error.to_string()))?;
    assert_eq!(inner.call_count(), 2);
    assert_eq!(
        guard_calls.load(Ordering::SeqCst),
        0,
        "被Steer取代的旧候选不应阻塞新输入"
    );
    assert_eq!(answers(&events), vec!["新输入后的安全回答"]);
    assert!(!visible_text(&events).contains("SECRET-DRAFT"));
    assert_no_legacy_assistant_events(&events);
    drop(agent);
    drop(conversations);
    let reopened = crate::memory::FileConversationStore::new(dir.path())?;
    let restored = echo_state::memory::conversation::restore_messages(
        &reopened.get_messages("steer-unchecked-restore").await?,
    )?;
    let draft = restored
        .iter()
        .find(|message| {
            message
                .assistant_blocks
                .as_ref()
                .is_some_and(|blocks| blocks.contains(&raw))
        })
        .ok_or_else(|| ReactError::Other("superseded raw candidate not archived".into()))?;
    assert!(
        draft
            .approved_assistant_blocks
            .as_ref()
            .is_some_and(Vec::is_empty),
        "原new runtime draft显式空许可，不能因Unknown冷重载而泄漏"
    );
    assert!(
        !draft
            .execution_status
            .as_ref()
            .is_some_and(|status| status.status == echo_core::agent::ExecutionStatusKind::Settled)
    );
    Ok(())
}

// EKO普通prepared模型的默认组合：最终Critic启用不等于隐藏已过Guard的工具前正文。
// 只替换外部模型/工具效果；所有事件、历史、Guard/Critic/Stop仍由原ReAct产生。
const DEFAULT_PROGRESS_TEXT: &str = "正在核对原工具的结果 🧭";
const DEFAULT_PROGRESS_FINAL: &str = "核对完成，保留原最终评审。";
const DEFAULT_PROGRESS_CRITIQUE: &str =
    r#"{"score":9.0,"passed":true,"feedback":"checked original final","suggestions":[]}"#;

struct DefaultProgressCritic {
    inner: crate::agent::critic::LlmCritic,
    answers: Arc<std::sync::Mutex<Vec<String>>>,
}
impl Critic for DefaultProgressCritic {
    fn critique<'a>(
        &'a self,
        task: &'a str,
        answer: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<Critique>> {
        Box::pin(async move {
            self.answers
                .lock()
                .map_err(|_| ReactError::Other("critic observation lock poisoned".into()))?
                .push(answer.to_string());
            self.inner.critique(task, answer, context).await
        })
    }
}

#[derive(Clone, Copy)]
enum DefaultProgressGuardAction {
    Pass,
    RejectProgress,
    TransformProgress,
}
struct DefaultProgressGuard {
    action: DefaultProgressGuardAction,
    checked: Arc<std::sync::Mutex<Vec<String>>>,
}
impl Guard for DefaultProgressGuard {
    fn name(&self) -> &str {
        "default-progress-original-output-guard"
    }
    fn check<'a>(
        &'a self,
        content: &'a str,
        direction: GuardDirection,
    ) -> BoxFuture<'a, Result<GuardResult>> {
        Box::pin(async move {
            if direction != GuardDirection::Output {
                return Ok(GuardResult::Pass);
            }
            self.checked
                .lock()
                .map_err(|_| ReactError::Other("guard observation lock poisoned".into()))?
                .push(content.to_string());
            if content == DEFAULT_PROGRESS_TEXT {
                match self.action {
                    DefaultProgressGuardAction::RejectProgress => {
                        return Ok(GuardResult::Block {
                            reason: "explicit output guard withheld progress".into(),
                        });
                    }
                    DefaultProgressGuardAction::TransformProgress => {
                        return Ok(GuardResult::Transform {
                            content: "Guard转换后的片段不借原raw身份发布".into(),
                            reasons: vec!["explicit progress transform".into()],
                        });
                    }
                    DefaultProgressGuardAction::Pass => {}
                }
            }
            Ok(GuardResult::Pass)
        })
    }
}

struct DefaultProgressToolGate {
    effects: Arc<AtomicUsize>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
impl Tool for DefaultProgressToolGate {
    fn name(&self) -> &str {
        "effect"
    }
    fn description(&self) -> &str {
        "Original Tool boundary with a test-owned completion barrier"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"step":{"type":"integer"}},"required":["step"]})
    }
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }
    fn execute(&self, _parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            self.effects.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            Ok(ToolResult::success("original gated effect result"))
        })
    }
}

fn default_progress_agent(
    llm: Arc<MockLlmClient>,
    tool: Box<dyn Tool>,
    action: DefaultProgressGuardAction,
    format: Option<crate::llm::ResponseFormat>,
    checked: Arc<std::sync::Mutex<Vec<String>>>,
    critiqued: Arc<std::sync::Mutex<Vec<String>>>,
) -> Result<ReactAgent> {
    let mut builder = ReactAgentBuilder::new()
        .llm_client(llm.clone())
        .model_profile(
            echo_core::llm::capabilities::ModelProfile::from_provider_name("gpt-4o", "openai"),
        )
        .tool(tool)
        .guard(Arc::new(DefaultProgressGuard { action, checked }))
        .system_prompt("按原协议继续工具执行；只在最终候选调用原评审。")
        .max_iterations(6);
    if let Some(format) = format {
        builder = builder.response_format(format);
    }
    let mut agent = builder.build()?;
    agent.config_mut().set_verifier_enabled(true);
    agent.config_mut().llm_max_retries = 0;
    agent.config_mut().llm_retry_delay_ms = 0;
    agent.config_mut().set_verifier_max_retries(0);
    // 与EKO Factory相同的owned LlmCritic构造和阈值；wrapper只记录真实评审入参。
    agent.set_owned_critic(
        "eko:model-generation",
        Arc::new(DefaultProgressCritic {
            inner: crate::agent::critic::LlmCritic::new(llm).with_pass_threshold(7.0),
            answers: critiqued,
        }),
    );
    Ok(agent)
}

fn default_progress_effect(effects: Arc<AtomicUsize>) -> Box<dyn Tool> {
    Box::new(EffectTool {
        name: "effect",
        effects,
        delay: Duration::ZERO,
    })
}

fn default_progress_script(phase: AssistantPhase) -> Result<Vec<StreamChunk>> {
    Ok(vec![
        native_delta(
            vec![block(
                "default-prose",
                "default-request-1",
                0,
                phase,
                DEFAULT_PROGRESS_TEXT,
            )],
            json!([call(0, "default-effect-1", "effect")]),
        )?,
        terminal("tool_calls"),
    ])
}

#[tokio::test]
async fn eko_default_critic_unknown_progress_precedes_each_original_tool() -> Result<()> {
    let mut signed = block(
        "original-reasoning",
        "default-request-1",
        0,
        AssistantPhase::Reasoning,
        "",
    );
    signed.reasoning = Some(ReasoningBlock::Signed {
        thinking: "原生公开推理摘要".into(),
        signature: "original-private-signature".into(),
    });
    let first = block(
        "default-prose",
        "default-request-1",
        1,
        AssistantPhase::Unknown,
        DEFAULT_PROGRESS_TEXT,
    );
    let second = block(
        "default-prose-2",
        "default-request-2",
        0,
        AssistantPhase::Unknown,
        "第一项已核对，继续原第二工具。",
    );
    let llm = Arc::new(
        MockLlmClient::new()
            .with_model_name("gpt-4o")
            .with_stream_script(vec![
                native_delta(
                    vec![signed.clone(), first.clone()],
                    json!([{"index":0,"id":"default-effect-1","type":"function","function":{"name":"effect","arguments":"{\"step\":1}"}}]),
                )?,
                terminal("tool_calls"),
            ])
            .with_stream_script(vec![
                native_delta(
                    vec![second.clone()],
                    // 不同合法读参数保留原缓存策略，确保两次都走真实工具入口屏障。
                    json!([{"index":0,"id":"default-effect-2","type":"function","function":{"name":"effect","arguments":"{\"step\":2}"}}]),
                )?,
                terminal("tool_calls"),
            ])
            .with_response(DEFAULT_PROGRESS_FINAL)
            .with_response(DEFAULT_PROGRESS_CRITIQUE),
    );
    let checked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let critiqued = Arc::new(std::sync::Mutex::new(Vec::new()));
    let effects = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let agent = default_progress_agent(
        llm.clone(),
        Box::new(DefaultProgressToolGate {
            effects: effects.clone(),
            entered: entered.clone(),
            release: release.clone(),
        }),
        DefaultProgressGuardAction::Pass,
        None,
        checked.clone(),
        critiqued.clone(),
    )?;
    let stops = Arc::new(AtomicUsize::new(0));
    let seen_stops = stops.clone();
    agent.hook_registry().write().await.set_programmatic_hook(
        "default-progress-stop",
        &[crate::skills::hooks::HookEvent::Stop],
        Arc::new(move |_context| {
            seen_stops.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { crate::skills::hooks::HookResult::allow() })
        }),
    );
    assert!(agent.config().verifier_enabled);
    assert!(agent.config().get_response_format().is_none());
    let mut stream = agent
        .run_stream_channel(
            StreamInit {
                text: "默认GUI进度组合反控".into(),
                message: None,
                label: String::new(),
                invocation: None,
            },
            StreamMode::Chat,
        )
        .await?;
    let mut events = Vec::new();
    let mut at_tools = Vec::new();
    for (call_id, expected) in [("default-effect-1", &first), ("default-effect-2", &second)] {
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(event) = stream.next().await {
                let event = event?;
                let arrived = matches!(&event, AgentEvent::ToolCall { call_id: actual, .. } if actual == call_id);
                events.push(event);
                if arrived { return Ok::<(), ReactError>(()); }
            }
            Err(ReactError::Other("original ToolCall was not published".into()))
        }).await.map_err(|_| ReactError::Other("original ToolCall barrier timed out".into()))??;
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .map_err(|_| {
                ReactError::Other(format!(
                    "original tool {call_id} did not reach its effect boundary; effects={}",
                    effects.load(Ordering::SeqCst)
                ))
            })?;
        // 先保存中间态再放行，最终失败断言不会把生产者留在测试屏障里。
        at_tools.push((
            blocks(&events).contains(&expected),
            answers(&events).is_empty(),
            critiqued
                .lock()
                .map_err(|_| ReactError::Other("critic lock poisoned".into()))?
                .is_empty(),
            stops.load(Ordering::SeqCst),
        ));
        release.notify_one();
    }
    let tail = tokio::time::timeout(Duration::from_secs(5), stream.collect::<Vec<_>>())
        .await
        .map_err(|_| ReactError::Other("original final settlement timed out".into()))?;
    events.extend(tail.into_iter().collect::<Result<Vec<_>>>()?);
    assert_eq!(
        at_tools,
        vec![(true, true, true, 0), (true, true, true, 0)],
        "默认Critic只审最终候选；每次真ToolCall之前应有原Unknown正文，不能用tool状态冒自然汇报"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 2);
    assert_eq!(answers(&events), vec![DEFAULT_PROGRESS_FINAL]);
    assert_eq!(
        *critiqued
            .lock()
            .map_err(|_| ReactError::Other("critic lock poisoned".into()))?,
        vec![DEFAULT_PROGRESS_FINAL.to_string()]
    );
    assert_eq!(stops.load(Ordering::SeqCst), 1);
    assert_eq!(
        llm.call_count(),
        4,
        "三个原主模型请求加一次原LlmCritic，无独立汇报模型loop"
    );
    let all = llm.all_calls();
    let next = all
        .get(1)
        .ok_or_else(|| ReactError::Other("post-tool original request missing".into()))?;
    assert!(
        next.iter()
            .any(|message| message.tool_call_id.as_deref() == Some("default-effect-1"))
    );
    let stored = next
        .iter()
        .find(|message| message.content.as_text().as_deref() == Some(DEFAULT_PROGRESS_TEXT))
        .ok_or_else(|| ReactError::Other("original prose absent from model history".into()))?;
    assert!(
        stored
            .assistant_blocks
            .as_ref()
            .is_some_and(|raw| raw.contains(&first))
    );
    assert!(
        stored
            .approved_assistant_blocks
            .as_ref()
            .is_some_and(|approved| approved.contains(&first))
    );
    assert!(
        blocks(&events).contains(&&signed),
        "原Signed reasoning必须保型，不退化为普通正文"
    );
    assert!(!visible_text(&events).contains("original-private-signature"));
    assert_eq!(
        blocks(&events)
            .iter()
            .filter(|candidate| **candidate == &first)
            .count(),
        1
    );
    assert_eq!(
        blocks(&events)
            .iter()
            .filter(|candidate| **candidate == &second)
            .count(),
        1
    );
    assert_no_legacy_assistant_events(&events);
    assert_one_settled_execution(&events)
}

#[tokio::test]
async fn eko_default_critic_explicit_final_with_tool_stays_private() -> Result<()> {
    for verifier_enabled in [false, true] {
        let model = MockLlmClient::new()
            .with_model_name("gpt-4o")
            .with_stream_script(default_progress_script(AssistantPhase::FinalAnswer)?)
            .with_response(DEFAULT_PROGRESS_FINAL);
        let llm = Arc::new(if verifier_enabled {
            model.with_response(DEFAULT_PROGRESS_CRITIQUE)
        } else {
            model
        });
        let effects = Arc::new(AtomicUsize::new(0));
        let critiqued = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut agent = default_progress_agent(
            llm.clone(),
            default_progress_effect(effects.clone()),
            DefaultProgressGuardAction::Pass,
            None,
            Arc::new(std::sync::Mutex::new(Vec::new())),
            critiqued.clone(),
        )?;
        agent.config_mut().set_verifier_enabled(verifier_enabled);
        let events = collect(&agent).await?;
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        assert!(
            !visible_text(&events).contains(DEFAULT_PROGRESS_TEXT),
            "显式Final+tools不能因verifier关闭而作为普通进度泄露候选"
        );
        assert_eq!(answers(&events), vec![DEFAULT_PROGRESS_FINAL]);
        let expected_critique = if verifier_enabled {
            vec![DEFAULT_PROGRESS_FINAL.to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            *critiqued
                .lock()
                .map_err(|_| ReactError::Other("critic lock poisoned".into()))?,
            expected_critique
        );
        assert_eq!(llm.call_count(), if verifier_enabled { 3 } else { 2 });
        let all = llm.all_calls();
        let prior = all
            .get(1)
            .into_iter()
            .flatten()
            .find(|message| message.content.as_text().as_deref() == Some(DEFAULT_PROGRESS_TEXT))
            .ok_or_else(|| ReactError::Other("raw final+tool history missing".into()))?;
        assert!(prior.assistant_blocks.as_ref().is_some_and(|raw| {
            raw.iter()
                .any(|item| item.phase == AssistantPhase::FinalAnswer)
        }));
        assert_eq!(
            prior.approved_assistant_blocks.as_deref(),
            Some([].as_slice())
        );
        assert_one_settled_execution(&events)?;
    }
    Ok(())
}

#[tokio::test]
async fn eko_default_critic_explicit_json_whole_output_holds_unknown_prose() -> Result<()> {
    for format in [
        crate::llm::ResponseFormat::JsonObject,
        crate::llm::ResponseFormat::json_schema(
            "whole-output",
            json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"]}),
        ),
    ] {
        let llm = Arc::new(
            MockLlmClient::new()
                .with_model_name("gpt-4o")
                .with_stream_script(default_progress_script(AssistantPhase::Unknown)?)
                .with_response(r#"{"ok":true}"#)
                .with_response(DEFAULT_PROGRESS_CRITIQUE),
        );
        let critiqued = Arc::new(std::sync::Mutex::new(Vec::new()));
        let agent = default_progress_agent(
            llm.clone(),
            default_progress_effect(Arc::new(AtomicUsize::new(0))),
            DefaultProgressGuardAction::Pass,
            Some(format.clone()),
            Arc::new(std::sync::Mutex::new(Vec::new())),
            critiqued.clone(),
        )?;
        let events = collect(&agent).await?;
        assert!(
            !visible_text(&events).contains(DEFAULT_PROGRESS_TEXT),
            "用户显式JSON整段输出保持hold，不跟随默认普通进度公开"
        );
        assert_eq!(answers(&events), vec![r#"{"ok":true}"#]);
        assert_eq!(
            *critiqued
                .lock()
                .map_err(|_| ReactError::Other("critic lock poisoned".into()))?,
            vec![r#"{"ok":true}"#.to_string()]
        );
        let first_format = llm.all_response_formats().into_iter().next().flatten();
        assert_eq!(
            serde_json::to_value(first_format)?,
            serde_json::to_value(format)?
        );
        assert_one_settled_execution(&events)?;
    }
    Ok(())
}

#[tokio::test]
async fn eko_default_critic_guard_reject_or_transform_never_exposes_raw_progress() -> Result<()> {
    for action in [
        DefaultProgressGuardAction::RejectProgress,
        DefaultProgressGuardAction::TransformProgress,
    ] {
        let llm = Arc::new(
            MockLlmClient::new()
                .with_model_name("gpt-4o")
                .with_stream_script(default_progress_script(AssistantPhase::Unknown)?)
                .with_response(DEFAULT_PROGRESS_FINAL)
                .with_response(DEFAULT_PROGRESS_CRITIQUE),
        );
        let checked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let critiqued = Arc::new(std::sync::Mutex::new(Vec::new()));
        let effects = Arc::new(AtomicUsize::new(0));
        let agent = default_progress_agent(
            llm.clone(),
            default_progress_effect(effects.clone()),
            action,
            None,
            checked.clone(),
            critiqued.clone(),
        )?;
        let events = collect(&agent).await?;
        assert!(
            checked
                .lock()
                .map_err(|_| ReactError::Other("guard lock poisoned".into()))?
                .iter()
                .any(|text| text == DEFAULT_PROGRESS_TEXT)
        );
        assert!(!visible_text(&events).contains(DEFAULT_PROGRESS_TEXT));
        assert!(
            !visible_text(&events).contains("Guard转换后的片段"),
            "不能用原raw的身份发布变更文本"
        );
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        assert_eq!(answers(&events), vec![DEFAULT_PROGRESS_FINAL]);
        let all = llm.all_calls();
        let raw = all
            .get(1)
            .into_iter()
            .flatten()
            .find(|message| message.content.as_text().as_deref() == Some(DEFAULT_PROGRESS_TEXT))
            .ok_or_else(|| ReactError::Other("guarded raw history missing".into()))?;
        assert_eq!(
            raw.approved_assistant_blocks.as_deref(),
            Some([].as_slice())
        );
        assert_one_settled_execution(&events)?;
    }
    Ok(())
}

#[tokio::test]
async fn eko_default_critic_stop_continue_keeps_prose_but_holds_rejected_final() -> Result<()> {
    let llm = Arc::new(
        MockLlmClient::new()
            .with_model_name("gpt-4o")
            .with_stream_script(default_progress_script(AssistantPhase::Unknown)?)
            .with_response("STOP-UNAPPROVED-FINAL")
            .with_response(DEFAULT_PROGRESS_CRITIQUE)
            .with_response(DEFAULT_PROGRESS_FINAL)
            .with_response(DEFAULT_PROGRESS_CRITIQUE),
    );
    let critiqued = Arc::new(std::sync::Mutex::new(Vec::new()));
    let agent = default_progress_agent(
        llm.clone(),
        default_progress_effect(Arc::new(AtomicUsize::new(0))),
        DefaultProgressGuardAction::Pass,
        None,
        Arc::new(std::sync::Mutex::new(Vec::new())),
        critiqued.clone(),
    )?;
    let stops = Arc::new(AtomicUsize::new(0));
    let seen = stops.clone();
    agent.hook_registry().write().await.set_programmatic_hook(
        "default-progress-continue",
        &[crate::skills::hooks::HookEvent::Stop],
        Arc::new(move |_context| {
            let first = seen.fetch_add(1, Ordering::SeqCst) == 0;
            Box::pin(async move {
                let mut result = crate::skills::hooks::HookResult::allow();
                if first {
                    result.continue_reason = Some("保持原执行继续".into());
                }
                result
            })
        }),
    );
    let events = collect(&agent).await?;
    assert!(visible_text(&events).contains(DEFAULT_PROGRESS_TEXT));
    assert!(!visible_text(&events).contains("STOP-UNAPPROVED-FINAL"));
    assert_eq!(answers(&events), vec![DEFAULT_PROGRESS_FINAL]);
    assert_eq!(
        *critiqued
            .lock()
            .map_err(|_| ReactError::Other("critic lock poisoned".into()))?,
        vec![
            "STOP-UNAPPROVED-FINAL".to_string(),
            DEFAULT_PROGRESS_FINAL.to_string()
        ]
    );
    assert_eq!(stops.load(Ordering::SeqCst), 2);
    assert_eq!(llm.call_count(), 5);
    assert_one_settled_execution(&events)
}

#[tokio::test]
async fn eko_default_critic_structured_final_error_precedes_critic_and_stop() -> Result<()> {
    let llm = Arc::new(
        MockLlmClient::new()
            .with_model_name("gpt-4o")
            .with_stream_script(default_progress_script(AssistantPhase::Unknown)?)
            .with_response(r#"{"ok":"wrong type"}"#),
    );
    let critiqued = Arc::new(std::sync::Mutex::new(Vec::new()));
    let agent = default_progress_agent(
        llm.clone(),
        default_progress_effect(Arc::new(AtomicUsize::new(0))),
        DefaultProgressGuardAction::Pass,
        Some(crate::llm::ResponseFormat::json_schema(
            "final-shape",
            json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"]}),
        )),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        critiqued.clone(),
    )?;
    let stops = Arc::new(AtomicUsize::new(0));
    let seen = stops.clone();
    agent.hook_registry().write().await.set_programmatic_hook(
        "default-progress-invalid-schema",
        &[crate::skills::hooks::HookEvent::Stop],
        Arc::new(move |_context| {
            seen.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { crate::skills::hooks::HookResult::allow() })
        }),
    );
    let raw = collect_raw(&agent, StreamMode::Chat).await?;
    assert!(
        raw.iter()
            .any(|event| event.is_err() || matches!(event, Ok(AgentEvent::Error { .. })))
    );
    let events: Vec<_> = raw
        .into_iter()
        .filter_map(std::result::Result::ok)
        .collect();
    assert!(answers(&events).is_empty());
    assert!(!visible_text(&events).contains("wrong type"));
    assert!(!visible_text(&events).contains(DEFAULT_PROGRESS_TEXT));
    assert!(
        critiqued
            .lock()
            .map_err(|_| ReactError::Other("critic lock poisoned".into()))?
            .is_empty()
    );
    assert_eq!(stops.load(Ordering::SeqCst), 0);
    assert_eq!(llm.call_count(), 2);
    Ok(())
}

#[tokio::test]
async fn eko_default_critic_veto_or_invalid_critique_cannot_publish_final() -> Result<()> {
    for critique in [
        r#"{"score":1.0,"passed":false,"feedback":"explicit veto","suggestions":[]}"#,
        "invalid critique JSON",
    ] {
        let llm = Arc::new(
            MockLlmClient::new()
                .with_model_name("gpt-4o")
                .with_stream_script(default_progress_script(AssistantPhase::Unknown)?)
                .with_response("CRITIC-UNAPPROVED-FINAL")
                .with_response(critique),
        );
        let critiqued = Arc::new(std::sync::Mutex::new(Vec::new()));
        let agent = default_progress_agent(
            llm.clone(),
            default_progress_effect(Arc::new(AtomicUsize::new(0))),
            DefaultProgressGuardAction::Pass,
            None,
            Arc::new(std::sync::Mutex::new(Vec::new())),
            critiqued.clone(),
        )?;
        let raw = collect_raw(&agent, StreamMode::Chat).await?;
        assert!(
            raw.iter()
                .any(|event| event.is_err() || matches!(event, Ok(AgentEvent::Error { .. })))
        );
        let events: Vec<_> = raw
            .into_iter()
            .filter_map(std::result::Result::ok)
            .collect();
        assert!(visible_text(&events).contains(DEFAULT_PROGRESS_TEXT));
        assert!(!visible_text(&events).contains("CRITIC-UNAPPROVED-FINAL"));
        assert!(answers(&events).is_empty());
        assert_eq!(
            *critiqued
                .lock()
                .map_err(|_| ReactError::Other("critic lock poisoned".into()))?,
            vec!["CRITIC-UNAPPROVED-FINAL".to_string()]
        );
        assert_eq!(llm.call_count(), 3);
    }
    Ok(())
}

#[tokio::test]
async fn eko_default_critic_named_progress_still_publishes_under_json_contract() -> Result<()> {
    for phase in [AssistantPhase::Commentary, AssistantPhase::PartialAnswer] {
        let llm = Arc::new(
            MockLlmClient::new()
                .with_model_name("gpt-4o")
                .with_stream_script(default_progress_script(phase)?)
                .with_response(r#"{"ok":true}"#)
                .with_response(DEFAULT_PROGRESS_CRITIQUE),
        );
        let agent = default_progress_agent(
            llm.clone(),
            default_progress_effect(Arc::new(AtomicUsize::new(0))),
            DefaultProgressGuardAction::Pass,
            Some(crate::llm::ResponseFormat::JsonObject),
            Arc::new(std::sync::Mutex::new(Vec::new())),
            Arc::new(std::sync::Mutex::new(Vec::new())),
        )?;
        let events = collect(&agent).await?;
        let prose = events.iter().position(|event| matches!(event, AgentEvent::AssistantMessage { block } if block.delta == DEFAULT_PROGRESS_TEXT && block.phase == phase));
        let tool = events.iter().position(|event| matches!(event, AgentEvent::ToolCall { call_id, .. } if call_id == "default-effect-1"));
        assert!(
            prose
                .zip(tool)
                .is_some_and(|(before, after)| before < after)
        );
        assert_eq!(answers(&events), vec![r#"{"ok":true}"#]);
        assert_one_settled_execution(&events)?;
    }
    Ok(())
}

#[tokio::test]
async fn eko_default_critic_late_empty_final_metadata_with_tool_stays_private() -> Result<()> {
    for verifier_enabled in [false, true] {
        let observations = late_phase_blocks(AssistantPhase::FinalAnswer, DEFAULT_PROGRESS_TEXT);
        let first = observations
            .first()
            .ok_or_else(|| ReactError::Other("late phase body missing".into()))?;
        assert_eq!(first.phase, AssistantPhase::Unknown);
        assert_eq!(
            super::phases::effective_assistant_phase(&observations, first),
            AssistantPhase::FinalAnswer,
            "发布权限必须沿原request/block最后明确phase，不只看早期Unknown"
        );
        assert!(
            observations.last().is_some_and(
                |last| last.delta.is_empty() && last.phase == AssistantPhase::FinalAnswer
            )
        );
        let mut script = observations
            .iter()
            .map(|observation| native_delta(vec![observation.clone()], Value::Null))
            .collect::<Result<Vec<_>>>()?;
        script.push(native_delta(
            Vec::new(),
            json!([call(0, "default-effect-1", "effect")]),
        )?);
        script.push(terminal("tool_calls"));
        let model = MockLlmClient::new()
            .with_model_name("gpt-4o")
            .with_stream_script(script)
            .with_response(DEFAULT_PROGRESS_FINAL);
        let llm = Arc::new(if verifier_enabled {
            model.with_response(DEFAULT_PROGRESS_CRITIQUE)
        } else {
            model
        });
        let effects = Arc::new(AtomicUsize::new(0));
        let critiqued = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut agent = default_progress_agent(
            llm.clone(),
            default_progress_effect(effects.clone()),
            DefaultProgressGuardAction::Pass,
            None,
            Arc::new(std::sync::Mutex::new(Vec::new())),
            critiqued.clone(),
        )?;
        agent.config_mut().set_verifier_enabled(verifier_enabled);
        let events = collect(&agent).await?;
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        assert!(
            !visible_text(&events).contains(DEFAULT_PROGRESS_TEXT),
            "晚到空delta FinalAnswer不能让早期Unknown绕过候选边界"
        );
        assert!(
            !blocks(&events)
                .iter()
                .any(|item| item.request_id == "r-late-phase"),
            "整个同块final候选的原观察都不能借中间publication发布"
        );
        assert_eq!(answers(&events), vec![DEFAULT_PROGRESS_FINAL]);
        let all = llm.all_calls();
        let prior = all
            .get(1)
            .into_iter()
            .flatten()
            .find(|message| message.content.as_text().as_deref() == Some(DEFAULT_PROGRESS_TEXT))
            .ok_or_else(|| ReactError::Other("late final raw history missing".into()))?;
        assert_eq!(
            prior.assistant_blocks.as_deref(),
            Some(observations.as_slice()),
            "原phase/空delta/来源不能改写"
        );
        assert_eq!(
            prior.approved_assistant_blocks.as_deref(),
            Some([].as_slice())
        );
        let expected_critique = if verifier_enabled {
            vec![DEFAULT_PROGRESS_FINAL.to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            *critiqued
                .lock()
                .map_err(|_| ReactError::Other("critic lock poisoned".into()))?,
            expected_critique
        );
        assert_eq!(llm.call_count(), if verifier_enabled { 3 } else { 2 });
        assert_one_settled_execution(&events)?;
    }
    Ok(())
}

#[tokio::test]
async fn eko_default_critic_original_continue_is_nonterminal_with_or_without_critic() -> Result<()>
{
    for verifier_enabled in [false, true] {
        let paused = block(
            "default-continue",
            "default-continue-request",
            0,
            AssistantPhase::Unknown,
            "原协议明确continue的普通正文，继续同一次执行。",
        );
        let model = MockLlmClient::new()
            .with_model_name("gpt-4o")
            .with_stream_script(vec![
                native_delta(vec![paused.clone()], Value::Null)?,
                terminal("continue"),
            ])
            .with_response(DEFAULT_PROGRESS_FINAL);
        let llm = Arc::new(if verifier_enabled {
            model.with_response(DEFAULT_PROGRESS_CRITIQUE)
        } else {
            model
        });
        let critiqued = Arc::new(std::sync::Mutex::new(Vec::new()));
        let effects = Arc::new(AtomicUsize::new(0));
        let mut agent = default_progress_agent(
            llm.clone(),
            default_progress_effect(effects.clone()),
            DefaultProgressGuardAction::Pass,
            None,
            Arc::new(std::sync::Mutex::new(Vec::new())),
            critiqued.clone(),
        )?;
        agent.config_mut().set_verifier_enabled(verifier_enabled);
        let stops = Arc::new(AtomicUsize::new(0));
        let seen = stops.clone();
        agent.hook_registry().write().await.set_programmatic_hook(
            "default-progress-protocol-continue",
            &[crate::skills::hooks::HookEvent::Stop],
            Arc::new(move |_context| {
                seen.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { crate::skills::hooks::HookResult::allow() })
            }),
        );
        let events = collect(&agent).await?;
        let progress_pos = events.iter().position(|event| {
            matches!(event,
            AgentEvent::AssistantMessage { block } if block == &paused)
        });
        let final_pos = events
            .iter()
            .position(|event| matches!(event, AgentEvent::FinalAnswer(_)));
        assert!(
            progress_pos
                .zip(final_pos)
                .is_some_and(|(progress, final_)| progress < final_),
            "原finish=continue是明确非终态，不能因最终Critic启用而吞其普通正文"
        );
        assert_eq!(
            effects.load(Ordering::SeqCst),
            0,
            "continue不伪造工具调用或效果"
        );
        assert_eq!(answers(&events), vec![DEFAULT_PROGRESS_FINAL]);
        assert_eq!(
            stops.load(Ordering::SeqCst),
            1,
            "continue轮不进入Stop，只有真正最终候选进入"
        );
        let expected_critique = if verifier_enabled {
            vec![DEFAULT_PROGRESS_FINAL.to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            *critiqued
                .lock()
                .map_err(|_| ReactError::Other("critic lock poisoned".into()))?,
            expected_critique
        );
        assert_eq!(llm.call_count(), if verifier_enabled { 3 } else { 2 });
        let all = llm.all_calls();
        let prior = all
            .get(1)
            .into_iter()
            .flatten()
            .find(|message| message.content.as_text().as_deref() == Some(paused.delta.as_str()))
            .ok_or_else(|| ReactError::Other("continue original history missing".into()))?;
        assert_eq!(
            prior.assistant_blocks.as_deref(),
            Some([paused.clone()].as_slice())
        );
        assert_eq!(
            prior.approved_assistant_blocks.as_deref(),
            Some([paused.clone()].as_slice())
        );
        assert_one_settled_execution(&events)?;
    }
    Ok(())
}
// 追加到现有native合同测试模块；仅替换LlmClient外边界，执行/历史/终态仍走原ReAct。
mod metadata_runtime_r01 {
    use super::*;
    use crate::error::LlmError;
    use crate::llm::types::{ProviderResponseMetadata, Role, Usage};
    use crate::llm::{ChatChunk, ChatRequest, ChatResponse, LlmApiProtocol, LlmClient};
    use echo_core::memory::{
        EnsureConversationProjectionRequest, ManagedConversationImport, NewConversation,
        TranscriptProjectionApplyStatus,
    };
    use echo_state::memory::conversation::{
        managed_import_projection_digests, project_messages, restore_messages,
        set_transcript_projection_meta,
    };
    use std::collections::VecDeque;
    use std::sync::Mutex;

    const SOURCE_USER: &str = "R01_ORIGINAL_FAILED_USER_INPUT";
    const PARTIAL_BODY: &str = "R01_UNAPPROVED_PARTIAL_BODY🙂";
    const PARTIAL_REASONING: &str = "R01_UNAPPROVED_REASONING";
    const PARTIAL_CALL: &str = "r01-unfinished-call";

    // 这是预设响应队列，不拥有运行状态，也不定义第二份metadata记录。
    enum Reply {
        Complete(Message),
        Incomplete(Message),
        // M2只在D2错误partial中出现，不能用一次成功Delta替代此控制。
        IncompleteTransition(Message, Box<Message>),
        RepeatedSnapshot(Message),
        ForeignSnapshot(Message, Box<ProviderResponseMetadata>),
    }

    struct Client {
        replies: Mutex<VecDeque<Reply>>,
        requests: Mutex<Vec<Vec<Message>>>,
        chat_calls: AtomicUsize,
        stream_calls: AtomicUsize,
    }

    impl Client {
        fn new(replies: Vec<Reply>) -> Self {
            Self {
                replies: Mutex::new(replies.into()),
                requests: Mutex::new(Vec::new()),
                chat_calls: AtomicUsize::new(0),
                stream_calls: AtomicUsize::new(0),
            }
        }

        fn take(&self, request: &ChatRequest) -> Result<Reply> {
            self.requests
                .lock()
                .map_err(|error| ReactError::Other(error.to_string()))?
                .push(request.messages.clone());
            self.replies
                .lock()
                .map_err(|error| ReactError::Other(error.to_string()))?
                .pop_front()
                .ok_or_else(|| ReactError::Other("unexpected extra metadata model call".into()))
        }

        fn requests(&self) -> Result<Vec<Vec<Message>>> {
            Ok(self
                .requests
                .lock()
                .map_err(|error| ReactError::Other(error.to_string()))?
                .clone())
        }

        fn call_count(&self) -> usize {
            self.chat_calls
                .load(Ordering::SeqCst)
                .saturating_add(self.stream_calls.load(Ordering::SeqCst))
        }
    }

    fn usage() -> Usage {
        Usage {
            prompt_tokens: Some(7),
            completion_tokens: Some(3),
            total_tokens: Some(10),
            ..Default::default()
        }
    }

    fn observation(request: &str, stop: &str) -> ProviderResponseMetadata {
        ProviderResponseMetadata {
            protocol: LlmApiProtocol::ChatCompletions,
            provider: "r01-fixture-provider".into(),
            request_id: Some(request.into()),
            model: Some(json!("r01-fixture-model")),
            stop_reason: Some(json!(stop)),
            stop_sequence: Some(Value::Null),
            status: None,
            incomplete_details: Some(Value::Null),
            usage: Some(json!({
                "prompt_tokens":7,"completion_tokens":3,"total_tokens":10,
                "completion_tokens_details":{"reasoning_tokens":1},
                "vendor_extension":{"cache":{"warm":false},"billable_units":2}
            })),
        }
    }

    fn message(text: &str, metadata: ProviderResponseMetadata) -> Message {
        let mut message = Message::assistant(text.to_string());
        message.provider_response = Some(metadata);
        message
    }

    fn tool_message(metadata: ProviderResponseMetadata) -> Result<Message> {
        let mut message = message("", metadata);
        message.tool_calls = Some(serde_json::from_value(json!([{
            "id":"r01-effect-call","type":"function",
            "function":{"name":"effect","arguments":"{}"}
        }]))?);
        Ok(message)
    }

    fn partial_message(metadata: ProviderResponseMetadata) -> Result<Message> {
        let mut message = message(PARTIAL_BODY, metadata);
        message.tool_calls = Some(serde_json::from_value(json!([{
            "id":PARTIAL_CALL,"type":"function",
            "function":{"name":"effect","arguments":"{"}
        }]))?);
        message.reasoning_blocks = Some(vec![ReasoningBlock::Signed {
            thinking: PARTIAL_REASONING.into(),
            signature: "r01-partial-signature".into(),
        }]);
        Ok(message)
    }

    fn delta(message: &Message) -> Result<DeltaMessage> {
        let calls = message
            .tool_calls
            .iter()
            .flatten()
            .enumerate()
            .map(|(index, call)| {
                Ok(json!({
                    "index":u32::try_from(index)
                        .map_err(|error| ReactError::Other(error.to_string()))?,
                    "id":call.id,"type":call.call_type,
                    "function":call.function,"provider_source":call.provider_source
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(serde_json::from_value(json!({
            "role":"assistant","content":message.text_content(),
            "tool_calls":if calls.is_empty(){Value::Null}else{json!(calls)},
            "reasoning_blocks":message.reasoning_blocks,
            "assistant_blocks":message.assistant_blocks,
            "provider_response":message.provider_response
        }))?)
    }

    fn metadata_delta(metadata: ProviderResponseMetadata) -> DeltaMessage {
        DeltaMessage {
            provider_response: Some(metadata),
            ..Default::default()
        }
    }

    impl LlmClient for Client {
        fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
            Box::pin(async move {
                self.chat_calls.fetch_add(1, Ordering::SeqCst);
                let message = match self.take(&request)? {
                    Reply::Complete(message) | Reply::RepeatedSnapshot(message) => message,
                    Reply::Incomplete(partial) => {
                        return Err(LlmError::IncompleteResponse {
                            partial: Box::new(partial),
                        }
                        .into());
                    }
                    Reply::IncompleteTransition(_, partial) => {
                        return Err(LlmError::IncompleteResponse { partial }.into());
                    }
                    Reply::ForeignSnapshot(_, _) => {
                        return Err(ReactError::Other(
                            "foreign snapshot fixture is stream-only".into(),
                        ));
                    }
                };
                let inner = MockLlmClient::new()
                    .with_response_usage(message.text_content().unwrap_or_default(), usage());
                let mut response = inner.chat(request).await?;
                response.finish_reason = Some(
                    if message
                        .tool_calls
                        .as_ref()
                        .is_some_and(|calls| !calls.is_empty())
                    {
                        "tool_calls"
                    } else {
                        "stop"
                    }
                    .into(),
                );
                // raw只作同一fixture响应的调试原件，不能与主Message的request/model冲突。
                if let Some(metadata) = message.provider_response.as_ref() {
                    if let Some(id) = metadata.request_id.as_ref() {
                        response.raw.id = id.clone();
                    }
                    response.raw.model = metadata
                        .model
                        .as_ref()
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                let mut native = message.clone();
                native.provider_response = None;
                native.execution_status = None;
                native.approved_assistant_blocks = None;
                response.raw.choices = vec![crate::llm::types::Choice {
                    message: native,
                    finish_reason: response.finish_reason.clone(),
                    index: Some(0),
                }];
                response.message = message;
                Ok(response)
            })
        }

        fn chat_stream(
            &self,
            request: ChatRequest,
        ) -> BoxFuture<'_, Result<futures::stream::BoxStream<'static, Result<ChatChunk>>>> {
            Box::pin(async move {
                self.stream_calls.fetch_add(1, Ordering::SeqCst);
                let reply = self.take(&request)?;
                let repeated = matches!(&reply, Reply::RepeatedSnapshot(_));
                let mut chunks = Vec::new();
                match reply {
                    Reply::Incomplete(partial) => {
                        chunks.push(Ok(ChatChunk {
                            delta: delta(&partial)?,
                            finish_reason: None,
                            usage: None,
                        }));
                        chunks.push(Err(LlmError::IncompleteResponse {
                            partial: Box::new(partial),
                        }
                        .into()));
                    }
                    Reply::IncompleteTransition(first, partial) => {
                        chunks.push(Ok(ChatChunk {
                            delta: delta(&first)?,
                            finish_reason: None,
                            usage: None,
                        }));
                        chunks.push(Err(LlmError::IncompleteResponse { partial }.into()));
                    }
                    Reply::ForeignSnapshot(original, foreign) => {
                        chunks.push(Ok(ChatChunk {
                            delta: delta(&original)?,
                            finish_reason: None,
                            usage: None,
                        }));
                        chunks.push(Ok(ChatChunk {
                            delta: metadata_delta(*foreign),
                            finish_reason: None,
                            usage: None,
                        }));
                        chunks.push(Ok(ChatChunk {
                            delta: DeltaMessage::default(),
                            finish_reason: Some("stop".into()),
                            usage: Some(usage()),
                        }));
                    }
                    Reply::Complete(message) | Reply::RepeatedSnapshot(message) => {
                        let full = message.provider_response.clone().ok_or_else(|| {
                            ReactError::Other(
                                "metadata response fixture omitted observation".into(),
                            )
                        })?;
                        let mut first = message.clone();
                        if let Some(metadata) = first.provider_response.as_mut() {
                            metadata.usage = None;
                        }
                        chunks.push(Ok(ChatChunk {
                            delta: delta(&first)?,
                            finish_reason: None,
                            usage: None,
                        }));
                        if repeated {
                            // 同一请求usage-only快照和最终完整快照都含相同归一用量。
                            chunks.push(Ok(ChatChunk {
                                delta: metadata_delta(full.clone()),
                                finish_reason: None,
                                usage: Some(usage()),
                            }));
                        }
                        chunks.push(Ok(ChatChunk {
                            delta: metadata_delta(full),
                            finish_reason: Some(
                                if message
                                    .tool_calls
                                    .as_ref()
                                    .is_some_and(|calls| !calls.is_empty())
                                {
                                    "tool_calls"
                                } else {
                                    "stop"
                                }
                                .into(),
                            ),
                            usage: Some(usage()),
                        }));
                    }
                }
                Ok(Box::pin(futures::stream::iter(chunks))
                    as futures::stream::BoxStream<'static, Result<ChatChunk>>)
            })
        }

        fn model_name(&self) -> &str {
            "r01-fixture-model"
        }

        fn provider_name(&self) -> Option<&str> {
            Some("r01-fixture-provider")
        }
    }

    async fn collect_invocation(
        agent: &ReactAgent,
        text: &str,
        invocation: Option<crate::agent::AgentInvocationContext>,
    ) -> Result<Vec<Result<AgentEvent>>> {
        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: text.to_string(),
                    message: Some(Message::user(text.to_string())),
                    label: String::new(),
                    invocation,
                },
                StreamMode::Chat,
            )
            .await?;
        // 先收原source到真实EOF，再在调用方检查错误/终态；不拿Err当EOF。
        tokio::time::timeout(Duration::from_secs(5), stream.collect())
            .await
            .map_err(|_| ReactError::Other("metadata source did not reach EOF".into()))
    }

    async fn context(agent: &ReactAgent) -> Vec<Message> {
        agent.context().lock().await.messages().to_vec()
    }

    fn assert_observation(messages: &[Message], expected: &ProviderResponseMetadata) {
        let matching = messages
            .iter()
            .filter_map(|message| message.provider_response.as_ref())
            .filter(|metadata| metadata.request_id == expected.request_id)
            .collect::<Vec<_>>();
        assert!(
            !matching.is_empty(),
            "original assistant lost provider response observation"
        );
        assert!(matching.into_iter().all(|metadata| metadata == expected));
    }

    fn assert_original_success_bindings(
        messages: &[Message],
        expected: &[ProviderResponseMetadata],
    ) -> Result<()> {
        let tool_metadata = expected
            .first()
            .ok_or_else(|| ReactError::Other("tool observation missing".into()))?;
        let stop_metadata = expected
            .get(1)
            .ok_or_else(|| ReactError::Other("stop observation missing".into()))?;
        let final_metadata = expected
            .get(2)
            .ok_or_else(|| ReactError::Other("final observation missing".into()))?;
        let tool = messages
            .iter()
            .find(|message| {
                message.role == Role::Assistant
                    && message
                        .tool_calls
                        .iter()
                        .flatten()
                        .any(|call| call.id == "r01-effect-call")
            })
            .ok_or_else(|| ReactError::Other("original tool assistant Message missing".into()))?;
        assert_eq!(tool.provider_response.as_ref(), Some(tool_metadata));
        let stop = messages
            .iter()
            .find(|message| {
                message.role == Role::Assistant
                    && matches!(
                        message.text_content().as_deref(),
                        Some("SECRET-DRAFT" | "已校验的安全正文")
                    )
            })
            .ok_or_else(|| ReactError::Other("Guard/Stop assistant body Message missing".into()))?;
        assert_eq!(stop.provider_response.as_ref(), Some(stop_metadata));
        assert!(!stop.is_provider_response_observation_only());
        let final_message = messages
            .iter()
            .find(|message| {
                message.role == Role::Assistant
                    && message.text_content().as_deref() == Some("metadata最终完成🙂")
            })
            .ok_or_else(|| {
                ReactError::Other("original final assistant body Message missing".into())
            })?;
        assert_eq!(
            final_message.provider_response.as_ref(),
            Some(final_metadata)
        );
        assert!(!final_message.is_provider_response_observation_only());
        assert!(
            !messages
                .iter()
                .any(|message| message.is_provider_response_observation_only()
                    && message
                        .provider_response
                        .as_ref()
                        .is_some_and(|metadata| expected
                            .iter()
                            .any(|wanted| wanted.request_id == metadata.request_id))),
            "successful responses cannot substitute extra observation-only rows for their body Messages"
        );
        Ok(())
    }

    fn assert_no_partial_history(messages: &[Message]) {
        for message in messages {
            assert!(
                !message
                    .text_content()
                    .unwrap_or_default()
                    .contains(PARTIAL_BODY)
            );
            assert!(
                !message
                    .reasoning_content
                    .as_deref()
                    .unwrap_or_default()
                    .contains(PARTIAL_REASONING)
            );
            assert!(message.tool_calls.as_ref().is_none_or(Vec::is_empty));
            for block in message
                .assistant_blocks
                .iter()
                .flatten()
                .chain(message.approved_assistant_blocks.iter().flatten())
            {
                assert!(!block.delta.contains(PARTIAL_BODY));
                assert!(
                    !matches!(&block.reasoning, Some(ReasoningBlock::Signed { thinking, .. }) if thinking.contains(PARTIAL_REASONING))
                );
            }
            assert!(!message.reasoning_blocks.iter().flatten().any(|block| {
                matches!(block, ReasoningBlock::Signed { thinking, .. } if thinking.contains(PARTIAL_REASONING))
            }));
        }
    }

    fn assert_failed_without_success(raw: &[Result<AgentEvent>]) -> Result<()> {
        let events = raw
            .iter()
            .filter_map(|item| item.as_ref().ok().cloned())
            .collect::<Vec<_>>();
        assert!(answers(&events).is_empty());
        let statuses = status_values(&events)?;
        assert_eq!(
            statuses
                .iter()
                .filter(|status| status
                    .get("data")
                    .and_then(|data| data.get("status"))
                    .and_then(Value::as_str)
                    == Some("failed"))
                .count(),
            1
        );
        assert!(!statuses.iter().any(|status| {
            status
                .get("data")
                .and_then(|data| data.get("status"))
                .and_then(Value::as_str)
                == Some("settled")
        }));
        Ok(())
    }

    #[tokio::test]
    async fn complete_response_metadata_survives_real_tool_guard_stop_stream_and_nonstream()
    -> Result<()> {
        for streaming in [true, false] {
            let root = fixture_dir()?;
            let conversation = if streaming {
                "r01-complete-stream"
            } else {
                "r01-complete-chat"
            };
            let expected = [
                observation("r01-tool", "tool_calls"),
                observation("r01-stop-continue", "stop"),
                observation("r01-final", "stop"),
            ];
            let client = Arc::new(Client::new(vec![
                Reply::Complete(tool_message(
                    expected
                        .first()
                        .ok_or_else(|| ReactError::Other("tool metadata missing".into()))?
                        .clone(),
                )?),
                Reply::Complete(message(
                    "SECRET-DRAFT",
                    expected
                        .get(1)
                        .ok_or_else(|| ReactError::Other("stop metadata missing".into()))?
                        .clone(),
                )),
                Reply::Complete(message(
                    "metadata最终完成🙂",
                    expected
                        .get(2)
                        .ok_or_else(|| ReactError::Other("final metadata missing".into()))?
                        .clone(),
                )),
            ]));
            let effects = Arc::new(AtomicUsize::new(0));
            let guard_calls = Arc::new(AtomicUsize::new(0));
            let stops = Arc::new(AtomicUsize::new(0));
            let conversations = Arc::new(crate::memory::FileConversationStore::new(root.path())?);
            let runtime = Arc::new(crate::state::FileRuntimeStateStore::new(root.path())?);
            let agent = ReactAgentBuilder::new()
                .llm_client(client.clone())
                .model_streaming(streaming)
                .conversation_id(conversation)
                .conversation_store(conversations.clone())
                .state_store(runtime.clone())
                .guard(Arc::new(TransformUnknownGuard(guard_calls.clone())))
                .tool(Box::new(EffectTool {
                    name: "effect",
                    effects: effects.clone(),
                    delay: Duration::ZERO,
                }))
                .max_iterations(6)
                .build()?;
            let stop_calls = stops.clone();
            agent.hook_registry().write().await.set_programmatic_hook(
                "r01-continue-once",
                &[crate::skills::hooks::HookEvent::Stop],
                Arc::new(move |_context| {
                    let first = stop_calls.fetch_add(1, Ordering::SeqCst) == 0;
                    Box::pin(async move {
                        let mut result = crate::skills::hooks::HookResult::allow();
                        if first {
                            result.continue_reason = Some("保留原响应观察后继续同一执行".into());
                        }
                        result
                    })
                }),
            );
            let events = collect(&agent).await?;
            assert_eq!(effects.load(Ordering::SeqCst), 1);
            assert_eq!(guard_calls.load(Ordering::SeqCst), 1);
            assert_eq!(stops.load(Ordering::SeqCst), 2);
            assert_eq!(client.call_count(), 3);
            assert_eq!(
                client.chat_calls.load(Ordering::SeqCst),
                if streaming { 0 } else { 3 }
            );
            assert_eq!(
                client.stream_calls.load(Ordering::SeqCst),
                if streaming { 3 } else { 0 }
            );
            assert_eq!(answers(&events), vec!["metadata最终完成🙂"]);
            assert_one_settled_execution(&events)?;
            let messages = context(&agent).await;
            for metadata in &expected {
                assert_observation(&messages, metadata);
            }
            assert_original_success_bindings(&messages, &expected)?;
            assert!(messages.iter().any(|message| {
                message
                    .tool_calls
                    .iter()
                    .flatten()
                    .any(|call| call.id == "r01-effect-call")
                    && message.provider_response.as_ref() == expected.first()
            }));
            agent.close().await?;
            drop(agent);
            drop(runtime);
            drop(conversations);
            let cold = crate::memory::FileConversationStore::new(root.path())?;
            let restored = restore_messages(&cold.get_messages(conversation).await?)?;
            for metadata in &expected {
                assert_observation(&restored, metadata);
            }
            assert_original_success_bindings(&restored, &expected)?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn incomplete_partial_is_nonreplayable_and_only_observation_survives_closed_file_import_and_fork()
    -> Result<()> {
        for streaming in [true, false] {
            let root = fixture_dir()?;
            let conversation = if streaming {
                "r01-incomplete-stream"
            } else {
                "r01-incomplete-chat"
            };
            let expected = observation("r01-incomplete", "length");
            let partial = partial_message(expected.clone())?;
            let client = Arc::new(Client::new(vec![
                Reply::Incomplete(partial.clone()),
                Reply::Complete(message(
                    "MUST_NOT_RETRY",
                    observation("r01-forbidden-retry", "stop"),
                )),
            ]));
            let effects = Arc::new(AtomicUsize::new(0));
            let conversations = Arc::new(crate::memory::FileConversationStore::new(root.path())?);
            let runtime = Arc::new(crate::state::FileRuntimeStateStore::new(root.path())?);
            let mut agent = ReactAgentBuilder::new()
                .llm_client(client.clone())
                .model_streaming(streaming)
                .conversation_id(conversation)
                .conversation_store(conversations.clone())
                .state_store(runtime.clone())
                .tool(Box::new(EffectTool {
                    name: "effect",
                    effects: effects.clone(),
                    delay: Duration::ZERO,
                }))
                .build()?;
            // 保留可用重试额度；不能靠关闭重试来掩盖IncompleteResponse分类错误。
            agent.config_mut().llm_max_retries = 2;
            agent.config_mut().llm_retry_delay_ms = 0;
            let raw = collect_invocation(&agent, SOURCE_USER, None).await?;
            assert_failed_without_success(&raw)?;
            assert!(
                raw.iter().any(|item| match item {
                    Err(error) => error.to_string().contains("Incomplete response"),
                    Ok(AgentEvent::Error { message, .. }) =>
                        message.contains("Incomplete response"),
                    _ => false,
                }),
                "runtime must retain the original bounded incomplete-response cause"
            );
            assert_eq!(
                client.call_count(),
                1,
                "incomplete response must not replay the request"
            );
            assert_eq!(
                client.stream_calls.load(Ordering::SeqCst),
                if streaming { 1 } else { 0 }
            );
            assert_eq!(
                client.chat_calls.load(Ordering::SeqCst),
                if streaming { 0 } else { 1 }
            );
            assert_eq!(effects.load(Ordering::SeqCst), 0);
            for item in &raw {
                match item {
                    Err(error) => assert!(!error.to_string().contains(PARTIAL_BODY)),
                    Ok(AgentEvent::Error { message, .. }) => {
                        assert!(!message.contains(PARTIAL_BODY))
                    }
                    Ok(AgentEvent::ToolCall { .. }) => {
                        return Err(ReactError::Other(
                            "incomplete tool fragment reached actual execution".into(),
                        ));
                    }
                    _ => {}
                }
            }
            let messages = context(&agent).await;
            assert_no_partial_history(&messages);
            let observed = messages
                .iter()
                .filter(|message| message.provider_response.as_ref() == Some(&expected))
                .collect::<Vec<_>>();
            assert_eq!(observed.len(), 1);
            assert!(
                observed
                    .iter()
                    .all(|message| message.is_provider_response_observation_only())
            );
            assert_eq!(
                observed
                    .first()
                    .map(|message| serde_json::to_value(*message))
                    .transpose()?,
                Some(serde_json::to_value(
                    Message::provider_response_observation(expected.clone())
                )?)
            );
            agent.close().await?;
            drop(agent);
            drop(runtime);
            drop(conversations);

            // 原Agent、RuntimeStore与ConversationStore均已关闭；只从真实File重开。
            let cold = crate::memory::FileConversationStore::new(root.path())?;
            let restored = restore_messages(&cold.get_messages(conversation).await?)?;
            assert_no_partial_history(&restored);
            assert!(
                restored
                    .iter()
                    .any(|message| message.is_provider_response_observation_only()
                        && message.provider_response.as_ref() == Some(&expected))
            );
            drop(cold);

            let fork_messages = {
                #[cfg(feature = "subagent")]
                {
                    // 原Fork继承入口及默认窗口，不扩history上界；GUI Fork仍独立验收。
                    let fork = crate::agent::subagent::SubagentContext::from_parent(
                        &[],
                        &restored,
                        None,
                        &crate::agent::subagent::ContextInheritance::fork_default(),
                    );
                    assert_no_partial_history(&fork.messages);
                    assert!(
                        fork.messages
                            .iter()
                            .any(|message| message.is_provider_response_observation_only()
                                && message.provider_response.as_ref() == Some(&expected))
                    );
                    fork.messages
                }
                #[cfg(not(feature = "subagent"))]
                {
                    restored.clone()
                }
            };
            for (target, branch_messages) in [
                ("r01-managed-import", &restored),
                ("r01-fork-import", &fork_messages),
            ] {
                let generation = target;
                let mut rows = project_messages(target, branch_messages)?;
                for (index, row) in rows.iter_mut().enumerate() {
                    let ordinal = u64::try_from(index)
                        .map_err(|error| ReactError::Other(error.to_string()))?;
                    set_transcript_projection_meta(row, generation, ordinal)?;
                }
                let expected_digests = managed_import_projection_digests(target, &rows)?;
                {
                    let store = crate::memory::FileConversationStore::new(root.path())?;
                    let authority = store
                        .ensure_projection_epoch(EnsureConversationProjectionRequest {
                            conversation: NewConversation {
                                conversation_id: target.into(),
                                user_id: "r01-test".into(),
                                agent_type: None,
                                title: None,
                            },
                            expected_tombstone_epoch: None,
                        })
                        .await?
                        .authority;
                    use crate::state::RuntimeStateStore;
                    let runtime = crate::state::FileRuntimeStateStore::new(root.path())?;
                    // 原managed-import CAS合同：先绑定导入前epoch的空runtime，不插入假历史。
                    let mut initial_checkpoint = crate::state::AgentCheckpoint::new(target);
                    initial_checkpoint.messages_json =
                        crate::state::AgentCheckpoint::serialize_payload(
                            Vec::new(),
                            Some(crate::state::TranscriptProjectionCheckpoint {
                                context_only: None,
                                generation_id: generation.to_string(),
                                next_ordinal: 0,
                                projected: Vec::new(),
                            }),
                        )?;
                    let initial = runtime
                        .compare_and_save_checkpoint(crate::state::RuntimeCheckpointCasRequest {
                            managed_import: None,
                            scope_id: target.to_string(),
                            runtime_state_id: target.to_string(),
                            conversation_epoch: Some(authority.epoch),
                            expected_scope_revision: 0,
                            expected_state_version:
                                crate::state::RuntimeStateExpectedVersion::Absent,
                            checkpoint: initial_checkpoint,
                        })
                        .await?;
                    assert_eq!(
                        initial.status,
                        crate::state::RuntimeCheckpointCasStatus::Applied
                    );
                    let expected_runtime_version = match initial.version {
                        crate::state::RuntimeStateVersion::Managed { revision } => {
                            crate::state::RuntimeStateExpectedVersion::Managed { revision }
                        }
                        _ => {
                            return Err(ReactError::Other(
                                "managed import initial runtime was not managed".into(),
                            ));
                        }
                    };
                    let import = ManagedConversationImport::prepare_for_generation(
                        target,
                        authority.epoch,
                        authority.revision,
                        generation,
                        rows,
                    )?;
                    let receipt = store.import_managed_messages(import.clone()).await?;
                    assert_eq!(receipt.status, TranscriptProjectionApplyStatus::Applied);
                    let digests = managed_import_projection_digests(target, &import.messages)?;
                    let cursor = crate::state::TranscriptProjectionCheckpoint {
                        context_only: None,
                        generation_id: generation.to_string(),
                        next_ordinal: u64::try_from(digests.len())
                            .map_err(|error| ReactError::Other(error.to_string()))?,
                        projected: digests
                            .into_iter()
                            .enumerate()
                            .map(|(ordinal, digest)| {
                                Ok(crate::state::TranscriptProjectionMessage {
                                    ordinal: u64::try_from(ordinal)
                                        .map_err(|error| ReactError::Other(error.to_string()))?,
                                    digest,
                                })
                            })
                            .collect::<Result<Vec<_>>>()?,
                    };
                    let mut checkpoint = crate::state::AgentCheckpoint::new(target);
                    checkpoint.messages_json = crate::state::AgentCheckpoint::serialize_payload(
                        restore_messages(&import.messages)?,
                        Some(cursor),
                    )?;
                    let bound = runtime
                        .compare_and_save_checkpoint(crate::state::RuntimeCheckpointCasRequest {
                            managed_import: Some(crate::state::ManagedImportEpochTransition {
                                request: import,
                                receipt: receipt.clone(),
                            }),
                            scope_id: target.to_string(),
                            runtime_state_id: target.to_string(),
                            conversation_epoch: Some(receipt.authority.epoch),
                            expected_scope_revision: initial.scope.revision,
                            expected_state_version: expected_runtime_version,
                            checkpoint,
                        })
                        .await?;
                    assert_eq!(
                        bound.status,
                        crate::state::RuntimeCheckpointCasStatus::Applied
                    );
                }
                {
                    let store = crate::memory::FileConversationStore::new(root.path())?;
                    let cold_rows = store.get_messages(target).await?;
                    assert_eq!(
                        managed_import_projection_digests(target, &cold_rows)?,
                        expected_digests
                    );
                    let cold_messages = restore_messages(&cold_rows)?;
                    assert_no_partial_history(&cold_messages);
                    assert!(
                        cold_messages
                            .iter()
                            .any(|message| message.is_provider_response_observation_only()
                                && message.provider_response.as_ref() == Some(&expected))
                    );
                }
                // 新generation的真实Agent再次生成请求，不能重放partial工具或正文。
                let next_client = Arc::new(Client::new(vec![Reply::Complete(message(
                    "导入后安全完成",
                    observation("r01-after-import", "stop"),
                ))]));
                let store = Arc::new(crate::memory::FileConversationStore::new(root.path())?);
                let state = Arc::new(crate::state::FileRuntimeStateStore::new(root.path())?);
                let next = ReactAgentBuilder::new()
                    .llm_client(next_client.clone())
                    .model_streaming(streaming)
                    .conversation_id(target)
                    .conversation_store(store.clone())
                    .state_store(state.clone())
                    .tool(Box::new(EffectTool {
                        name: "effect",
                        effects: effects.clone(),
                        delay: Duration::ZERO,
                    }))
                    .build()?;
                let original_user_retained = branch_messages.iter().any(|message| {
                    message.role == Role::User
                        && message.text_content().as_deref() == Some(SOURCE_USER)
                });
                let invocation = crate::agent::AgentInvocationContext {
                    runtime_state_id: Some(target.to_string()),
                    transcript_generation_id: Some(target.to_string()),
                    // 完整导入历史已由同一CAS绑定并从checkpoint冷hydrate，不能再次追加。
                    history: None,
                    runtime: Some(crate::tools::ExternalRunContext {
                        conversation_id: Some(target.to_string()),
                        run_id: Some(format!("r01-new-run:{target}")),
                        turn_id: Some(format!("r01-new-turn:{target}")),
                        execution_id: Some(format!("r01-new-execution:{target}")),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                let events = collect_invocation(&next, "R01_IMPORTED_FOLLOWUP", Some(invocation))
                    .await?
                    .into_iter()
                    .collect::<Result<Vec<_>>>()?;
                assert_eq!(
                    answers(&events),
                    vec!["导入后安全完成"],
                    "managed followup target={target} streaming={streaming} calls={} events={events:#?}",
                    next_client.call_count()
                );
                assert_one_settled_execution(&events)?;
                assert_eq!(effects.load(Ordering::SeqCst), 0);
                let requests = next_client.requests()?;
                assert_eq!(requests.len(), 1);
                for request in &requests {
                    assert_no_partial_history(request);
                    if original_user_retained {
                        assert!(
                            request.iter().any(|message| message.role == Role::User
                                && message.text_content().as_deref() == Some(SOURCE_USER)),
                            "empty import history must not make partial exclusion vacuously pass"
                        );
                    }
                }
                assert_observation(&context(&next).await, &expected);
                next.close().await?;
                drop(next);
                drop(store);
                drop(state);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn usage_only_and_full_observation_snapshots_do_not_double_normalized_usage() -> Result<()>
    {
        let expected = observation("r01-usage", "stop");
        let client = Arc::new(Client::new(vec![Reply::RepeatedSnapshot(message(
            "用量仅记一次",
            expected.clone(),
        ))]));
        let agent = ReactAgentBuilder::new()
            .llm_client(client.clone())
            .model_streaming(true)
            .build()?;
        let events = collect(&agent).await?;
        assert_eq!(answers(&events), vec!["用量仅记一次"]);
        assert_one_settled_execution(&events)?;
        let usage = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::LlmUsage {
                    prompt_tokens,
                    completion_tokens,
                    total_tokens,
                    usage_reported: true,
                    ..
                } => Some((*prompt_tokens, *completion_tokens, *total_tokens)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(usage, vec![(7, 3, 10)]);
        assert_eq!(client.stream_calls.load(Ordering::SeqCst), 1);
        assert_eq!(client.chat_calls.load(Ordering::SeqCst), 0);
        let messages = context(&agent).await;
        assert_observation(&messages, &expected);
        let original = messages
            .iter()
            .find(|message| {
                message.role == Role::Assistant
                    && message.text_content().as_deref() == Some("用量仅记一次")
            })
            .ok_or_else(|| ReactError::Other("usage response body Message missing".into()))?;
        assert_eq!(original.provider_response.as_ref(), Some(&expected));
        assert!(!original.is_provider_response_observation_only());
        agent.close().await?;
        Ok(())
    }

    #[tokio::test]
    async fn foreign_request_observation_is_rejected_without_overwriting_the_bound_snapshot()
    -> Result<()> {
        let expected = observation("r01-bound", "stop");
        let foreign = observation("r01-foreign", "stop");
        let client = Arc::new(Client::new(vec![Reply::ForeignSnapshot(
            message("未确认草稿", expected.clone()),
            Box::new(foreign.clone()),
        )]));
        let mut agent = ReactAgentBuilder::new()
            .llm_client(client.clone())
            .model_streaming(true)
            .build()?;
        agent.config_mut().llm_max_retries = 2;
        agent.config_mut().llm_retry_delay_ms = 0;
        let raw = collect_raw(&agent, StreamMode::Chat).await?;
        assert_failed_without_success(&raw)?;
        assert_eq!(client.call_count(), 1);
        assert_eq!(client.stream_calls.load(Ordering::SeqCst), 1);
        assert_eq!(client.chat_calls.load(Ordering::SeqCst), 0);
        let messages = context(&agent).await;
        assert_observation(&messages, &expected);
        assert!(!messages.iter().any(|message| {
            message
                .provider_response
                .as_ref()
                .is_some_and(|metadata| metadata.request_id == foreign.request_id)
        }));
        assert!(
            messages
                .iter()
                .filter(|message| message.provider_response.as_ref() == Some(&expected))
                .all(|message| message.is_provider_response_observation_only())
        );
        agent.close().await?;
        Ok(())
    }
    #[tokio::test]
    async fn latest_partial_metadata_is_retained_for_same_request_and_foreign_partial_keeps_bound_metadata()
    -> Result<()> {
        for foreign in [false, true] {
            let root = fixture_dir()?;
            let scope = if foreign {
                "r04-metadata-foreign-partial"
            } else {
                "r04-metadata-latest-partial"
            };
            let mut first = observation("r04-bound-response", "length");
            first.incomplete_details = None;
            first.usage = Some(json!({"snapshot":"M1","tokens":1}));
            let mut latest = first.clone();
            latest.incomplete_details = Some(json!({"reason":"max_tokens","snapshot":"M2"}));
            latest.usage = Some(json!({"snapshot":"M2","tokens":7,"nested":{"billable":3}}));
            if foreign {
                latest.request_id = Some("r04-foreign-response".into());
            }
            let expected = if foreign {
                first.clone()
            } else {
                latest.clone()
            };
            let client = Arc::new(Client::new(vec![
                Reply::IncompleteTransition(
                    partial_message(first.clone())?,
                    Box::new(partial_message(latest.clone())?),
                ),
                Reply::Complete(message(
                    "MUST_NOT_RETRY_R04",
                    observation("r04-retry-must-not-run", "stop"),
                )),
            ]));
            let effects = Arc::new(AtomicUsize::new(0));
            let conversations = Arc::new(crate::memory::FileConversationStore::new(root.path())?);
            let state = Arc::new(crate::state::FileRuntimeStateStore::new(root.path())?);
            let mut agent = ReactAgentBuilder::new()
                .llm_client(client.clone())
                .model_streaming(true)
                .conversation_id(scope)
                .conversation_store(conversations.clone())
                .state_store(state.clone())
                .tool(Box::new(EffectTool {
                    name: "effect",
                    effects: effects.clone(),
                    delay: Duration::ZERO,
                }))
                .build()?;
            agent.config_mut().llm_max_retries = 2;
            agent.config_mut().llm_retry_delay_ms = 0;
            let raw = collect_invocation(&agent, "R04_PARTIAL_METADATA_INPUT", None).await?;
            let messages = context(&agent).await;
            // 先结束原失败owner/关闭所有File handle，再作会故意RED的比较。
            let closed = agent.close().await;
            drop(agent);
            drop(state);
            drop(conversations);
            assert_failed_without_success(&raw)?;
            assert!(
                raw.iter().any(|item| match item {
                    Err(error) => error.to_string().contains("Incomplete response"),
                    Ok(AgentEvent::Error { message, .. }) =>
                        message.contains("Incomplete response"),
                    _ => false,
                }),
                "foreign metadata rejection must not replace the original bounded incomplete cause"
            );
            for item in &raw {
                match item {
                    Err(error) => assert!(!error.to_string().contains(PARTIAL_BODY)),
                    Ok(AgentEvent::Error { message, .. }) => {
                        assert!(!message.contains(PARTIAL_BODY))
                    }
                    _ => {}
                }
            }
            closed?;
            assert_eq!(client.call_count(), 1);
            assert_eq!(client.stream_calls.load(Ordering::SeqCst), 1);
            assert_eq!(client.chat_calls.load(Ordering::SeqCst), 0);
            assert_eq!(effects.load(Ordering::SeqCst), 0);
            assert_no_partial_history(&messages);
            let observed = messages
                .iter()
                .filter(|message| message.provider_response.is_some())
                .collect::<Vec<_>>();
            assert_eq!(observed.len(), 1);
            assert!(
                observed
                    .iter()
                    .all(|message| message.is_provider_response_observation_only())
            );
            assert_eq!(
                observed
                    .first()
                    .and_then(|message| message.provider_response.as_ref()),
                Some(&expected)
            );
            if foreign {
                assert_ne!(
                    Some(&latest),
                    observed
                        .first()
                        .and_then(|message| message.provider_response.as_ref())
                );
            }
            let cold = crate::memory::FileConversationStore::new(root.path())?;
            let restored = restore_messages(&cold.get_messages(scope).await?)?;
            assert_no_partial_history(&restored);
            let cold_observed = restored
                .iter()
                .filter(|message| message.provider_response.is_some())
                .collect::<Vec<_>>();
            assert_eq!(cold_observed.len(), 1);
            assert!(
                cold_observed
                    .iter()
                    .all(|message| message.is_provider_response_observation_only())
            );
            assert_eq!(
                cold_observed
                    .first()
                    .and_then(|message| message.provider_response.as_ref()),
                Some(&expected)
            );
        }
        Ok(())
    }
}
