//! 真实ReAct publisher→既有AnthropicClient→第二HTTP。只用已有public API。
//! 通过子测试进程的标准HTTP proxy指向loopback，保官方DeepSeek URL判定，
//! 不改全局环境、不增加生产client setter、不向真实服务发请求。
use crate::agent::{Agent, AgentEvent, ReactAgentBuilder};
use crate::error::{ReactError, Result};
use crate::llm::{ChatChunk, ChatRequest, ChatResponse, LlmApiProtocol, LlmClient, LlmConfig};
use crate::tools::{Tool, ToolParameters, ToolResult};
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

struct ActualTool {
    calls: Arc<AtomicUsize>,
    fail: bool,
}
impl Tool for ActualTool {
    fn name(&self) -> &str {
        "ordinary"
    }
    fn description(&self) -> &str {
        "Observe actual ToolResult publisher"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{}})
    }
    fn execute<'a>(&'a self, _: ToolParameters) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(if self.fail {
                ToolResult::error("actual failure")
            } else {
                ToolResult::success("[Error] is ordinary text; actual success")
            })
        })
    }
}

async fn child(streaming: bool, fail: bool) -> Result<()> {
    let endpoint = std::env::var("EKO_CONSUMER_DS_TOOL_ENDPOINT")
        .map_err(|error| ReactError::Other(error.to_string()))?;
    let config = LlmConfig::for_provider(
        "deepseek",
        endpoint,
        "fixture-only-not-a-live-token",
        "deepseek-flash",
        LlmApiProtocol::Anthropic,
    )?;
    let client = Arc::new(RealClientRecorder {
        inner: Arc::from(config.build_client()?),
        requests: Mutex::new(Vec::new()),
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let mut agent = ReactAgentBuilder::new()
        .configured_llm_client(client.clone(), config)
        .model_streaming(streaming)
        .auto_project_rules(false)
        .tool(Box::new(ActualTool {
            calls: calls.clone(),
            fail,
        }))
        .build()?;
    assert!(
        agent
            .llm_client()
            .is_some_and(|client| client.capabilities().tool_support)
    );
    // None只来自合法外部历史。managed入口先恢复/重置，公开invocation.history随后入ctx。
    // 不用load_messages(None scope)后再换runtime identity，让新空checkpoint覆盖它。
    let history = vec![crate::llm::types::Message::user("historical request".into()),serde_json::from_value(json!({"role":"assistant","content":null,
        "tool_calls":[{"id":"history-call","type":"function","function":{"name":"ordinary","arguments":"{}"}}]}))
        .map_err(|error|ReactError::Other(error.to_string()))?,
        crate::llm::types::Message::tool_result("history-call".into(),"ordinary".into(),"known historical ordinary result".into())];
    let root = super::consumer_test_artifacts::root()?;
    let directory =
        tempfile::tempdir_in(root).map_err(|error| ReactError::Other(error.to_string()))?;
    let conversations = Arc::new(crate::memory::FileConversationStore::new(
        directory.path().join("conversations"),
    )?);
    let runtime = Arc::new(crate::state::FileRuntimeStateStore::new(
        directory.path().join("runtime"),
    )?);
    agent.set_conversation_id("publisher-observation");
    agent.set_conversation_store(conversations.clone());
    agent.set_state_store(runtime.clone());
    let raw = agent
        .chat_stream_message_with_invocation_context(
            crate::llm::types::Message::user("execute ordinary tool, then finish".into()),
            crate::agent::CancellationToken::new(),
            echo_core::agent::AgentInvocationContext {
                history: Some(history),
                ..Default::default()
            },
        )
        .await?
        .collect::<Vec<_>>()
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "不可重复或绕过真实工具");
    {
        let requests = client
            .requests
            .lock()
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert_eq!(
            requests.len(),
            2,
            "必须真实到达下一请求的生产adapter，而非手造默认None"
        );
        let first = requests
            .first()
            .ok_or_else(|| ReactError::Other("first request missing".into()))?;
        let first_history = first
            .messages
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("history-call"))
            .ok_or_else(|| {
                ReactError::Other(
                    "external history did not survive managed hydration before first request"
                        .into(),
                )
            })?;
        assert!(
            serde_json::to_value(first_history)
                .map_err(|error| ReactError::Other(error.to_string()))?
                .get("tool_result_is_error")
                .is_none()
        );
        let next = requests
            .get(1)
            .ok_or_else(|| ReactError::Other("next client request missing".into()))?;
        let actual = next
            .messages
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("producer-call"))
            .ok_or_else(|| ReactError::Other("actual publisher result missing".into()))?;
        let value =
            serde_json::to_value(actual).map_err(|error| ReactError::Other(error.to_string()))?;
        assert_eq!(
            value.get("tool_result_is_error"),
            Some(&json!(fail)),
            "Core观察必须保留，不为降方言改写"
        );
        let historical = next
            .messages
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("history-call"))
            .ok_or_else(|| ReactError::Other("unknown historical observation missing".into()))?;
        assert!(
            serde_json::to_value(historical)
                .map_err(|error| ReactError::Other(error.to_string()))?
                .get("tool_result_is_error")
                .is_none()
        );
        if fail {
            assert!(
                !raw.iter()
                    .any(|event| matches!(event, Ok(AgentEvent::FinalAnswer(_))))
            );
            assert!(
                raw.iter()
                    .any(|event| event.is_err() || matches!(event, Ok(AgentEvent::Error { .. })))
            );
            assert!(
                raw.iter().any(|event| match event {
                    Err(error) =>
                        error.to_string().contains("DeepSeek")
                            && error.to_string().contains("is_error"),
                    Ok(AgentEvent::Error { message, .. }) =>
                        message.contains("DeepSeek") && message.contains("is_error"),
                    _ => false,
                }),
                "必须是原DeepSeek错误观察准入拒绝，不能由其它失败假过"
            );
        } else {
            assert!(
                raw.iter().all(Result::is_ok),
                "Some(false)回填被拒：{raw:#?}"
            );
            assert_eq!(
                raw.iter()
                    .filter(|event| matches!(event,Ok(AgentEvent::FinalAnswer(text)) if text=="OK"))
                    .count(),
                1
            );
        }
    }
    agent.close().await?;
    drop(agent);
    drop(conversations);
    drop(runtime);
    use crate::memory::ConversationStore;
    let reopened =
        crate::memory::FileConversationStore::new(directory.path().join("conversations"))?;
    let restored =
        crate::memory::restore_messages(&reopened.get_messages("publisher-observation").await?)?;
    let actual = restored
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("producer-call"))
        .ok_or_else(|| ReactError::Other("cold publisher result missing".into()))?;
    assert_eq!(
        serde_json::to_value(actual)
            .map_err(|error| ReactError::Other(error.to_string()))?
            .get("tool_result_is_error"),
        Some(&json!(fail))
    );
    Ok(())
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

fn body(streaming: bool, first: bool) -> String {
    let content = if first {
        vec![json!({"type":"tool_use","id":"producer-call","name":"ordinary","input":{}})]
    } else {
        vec![json!({"type":"text","text":"OK"})]
    };
    let reason = if first { "tool_use" } else { "end_turn" };
    if !streaming {
        return json!({"id":if first {"native-1"} else {"native-2"},"type":"message","role":"assistant","model":"deepseek-flash","content":content,"stop_reason":reason,"usage":{"input_tokens":1,"output_tokens":1}}).to_string();
    }
    let event = |name: &str, value: Value| format!("event: {name}\ndata: {value}\n\n");
    let mut output = event(
        "message_start",
        json!({"type":"message_start","message":{"id":if first {"native-1"} else {"native-2"},"type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}),
    );
    for (index, item) in content.into_iter().enumerate() {
        output.push_str(&event(
            "content_block_start",
            json!({"type":"content_block_start","index":index,"content_block":item}),
        ));
        output.push_str(&event(
            "content_block_stop",
            json!({"type":"content_block_stop","index":index}),
        ));
    }
    output.push_str(&event(
        "message_delta",
        json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":1}}),
    ));
    output.push_str(&event("message_stop", json!({"type":"message_stop"})));
    output
}

async fn parent(name: &str, fail: bool) -> Result<()> {
    if let Ok(mode) = std::env::var("EKO_CONSUMER_DS_TOOL_CHILD_MODE") {
        return child(mode == "stream", fail).await;
    }
    for streaming in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let proxy = format!(
            "http://{}",
            listener
                .local_addr()
                .map_err(|error| ReactError::Other(error.to_string()))?
        );
        let endpoint = format!(
            "http://api.deepseek.com:{}/anthropic/v1/messages",
            listener
                .local_addr()
                .map_err(|error| ReactError::Other(error.to_string()))?
                .port()
        );
        let module = module_path!()
            .split_once("::")
            .map(|(_, module)| module)
            .unwrap_or(module_path!());
        let mut command = tokio::process::Command::new(
            std::env::current_exe().map_err(|error| ReactError::Other(error.to_string()))?,
        );
        command
            .args(["--exact", &format!("{module}::{name}"), "--nocapture"])
            .env(
                "EKO_CONSUMER_DS_TOOL_CHILD_MODE",
                if streaming { "stream" } else { "nonstream" },
            )
            .env("EKO_CONSUMER_DS_TOOL_ENDPOINT", endpoint)
            .env("HTTP_PROXY", &proxy)
            .env("http_proxy", &proxy)
            .env("HTTPS_PROXY", &proxy)
            .env("https_proxy", &proxy)
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .env_remove("ALL_PROXY")
            .env_remove("all_proxy")
            .kill_on_drop(true);
        let root = super::consumer_test_artifacts::root()?;
        command.env("EKO_CONSUMER_TEST_ROOT", &root);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| ReactError::Other(error.to_string()))?
            .as_nanos();
        let diagnostics = root.join(format!(
            "fixture-r02-{name}-{}-{}-{stamp}",
            std::process::id(),
            if streaming { "stream" } else { "nonstream" }
        ));
        std::fs::create_dir_all(&diagnostics)
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let stdout_path = diagnostics.join("child.stdout.log");
        let stderr_path = diagnostics.join("child.stderr.log");
        command
            .stdout(std::process::Stdio::from(
                std::fs::File::create(&stdout_path)
                    .map_err(|error| ReactError::Other(error.to_string()))?,
            ))
            .stderr(std::process::Stdio::from(
                std::fs::File::create(&stderr_path)
                    .map_err(|error| ReactError::Other(error.to_string()))?,
            ));
        let http_seen = Arc::new(AtomicUsize::new(0));
        let connection_seen = Arc::new(AtomicUsize::new(0));
        let server_http = http_seen.clone();
        let server_connections = connection_seen.clone();
        let server = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(10),async move {
                let mut requests=Vec::new();
                for ordinal in 0..if fail {1} else {2} {
                    let (mut socket,_)=listener.accept().await?;
                    server_connections.fetch_add(1,Ordering::SeqCst);
                    let (line,request)=read_request(&mut socket).await?;
                    server_http.fetch_add(1,Ordering::SeqCst);
                    assert!(line.contains("http://api.deepseek.com:")&&line.contains("/anthropic/"),"must use loopback proxy while preserving official URL dialect identity");
                    let payload=body(streaming,ordinal==0);let content_type=if streaming {"text/event-stream"} else {"application/json"};
                    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",payload.len()).as_bytes()).await?;
                    requests.push(request);
                }
                if fail
                    && let Ok(extra)=tokio::time::timeout(std::time::Duration::from_millis(200),listener.accept()).await {
                    let (socket,_)=extra?;server_connections.fetch_add(1,Ordering::SeqCst);drop(socket);
                    return Err(std::io::Error::other("Some(true) opened an unexpected second native connection"));
                }
                Ok::<_,std::io::Error>(requests)
            }).await.map_err(std::io::Error::other)?
        });
        let mut process = match command.spawn() {
            Ok(process) => process,
            Err(error) => {
                server.abort();
                let _ = server.await;
                return Err(ReactError::Other(format!(
                    "child spawn failed: {error}; diagnostics={}",
                    diagnostics.display()
                )));
            }
        };
        let (status, wait_error, timed_out) =
            match tokio::time::timeout(std::time::Duration::from_secs(15), process.wait()).await {
                Ok(Ok(status)) => (Some(status), None, false),
                Ok(Err(error)) => {
                    let _ = process.kill().await;
                    (process.wait().await.ok(), Some(error.to_string()), false)
                }
                Err(error) => {
                    let _ = process.kill().await;
                    (process.wait().await.ok(), Some(error.to_string()), true)
                }
            };
        let stdout =
            std::fs::read(&stdout_path).map_err(|error| ReactError::Other(error.to_string()))?;
        let stderr =
            std::fs::read(&stderr_path).map_err(|error| ReactError::Other(error.to_string()))?;
        let selected_one = String::from_utf8_lossy(&stdout).contains("running 1 test");
        let child_success = status.as_ref().is_some_and(|status| status.success())
            && !timed_out
            && wait_error.is_none()
            && selected_one;
        // report始终是此处固定构造且不被替换的JSON对象，后续字符串键写入不会索引标量。
        let mut report = json!({"streaming":streaming,"fail":fail,"exit_code":status.as_ref().and_then(|status|status.code()),
            "status":format!("{status:?}"),"wait_error":wait_error,"timed_out":timed_out,"selected_one":selected_one,
            "http_count_before_server_join":http_seen.load(Ordering::SeqCst),"connections_before_server_join":connection_seen.load(Ordering::SeqCst),
            "stdout":stdout_path.display().to_string(),"stderr":stderr_path.display().to_string()});
        // 必须先保存/显示真实child首错，再检查server；accept超时不能覆盖它。
        std::fs::write(diagnostics.join("result.json"), report.to_string())
            .map_err(|error| ReactError::Other(error.to_string()))?;
        eprintln!(
            "CHILD_RESULT {report}\nCHILD_STDOUT:\n{}\nCHILD_STDERR:\n{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        );
        if !child_success {
            server.abort();
            let joined = server.await;
            report["server_outcome"] = json!(format!("aborted after child failure: {joined:?}"));
            report["http_count_final"] = json!(http_seen.load(Ordering::SeqCst));
            std::fs::write(diagnostics.join("result.json"), report.to_string())
                .map_err(|error| ReactError::Other(error.to_string()))?;
            return Err(ReactError::Other(format!(
                "child first failure, native HTTP={}, diagnostics={}; stderr={}",
                http_seen.load(Ordering::SeqCst),
                diagnostics.display(),
                String::from_utf8_lossy(&stderr)
            )));
        }
        let server_result = server.await;
        report["server_outcome"] = json!(format!("{server_result:?}"));
        report["http_count_final"] = json!(http_seen.load(Ordering::SeqCst));
        std::fs::write(diagnostics.join("result.json"), report.to_string())
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let requests = server_result
            .map_err(|error| {
                ReactError::Other(format!(
                    "server task failed after child output saved: {error}; {report}"
                ))
            })?
            .map_err(|error| {
                ReactError::Other(format!(
                    "server failed after child output saved: {error}; {report}"
                ))
            })?;
        assert_eq!(requests.len(), if fail { 1 } else { 2 });
        if !fail {
            let next = requests
                .get(1)
                .ok_or_else(|| ReactError::Other("second native request missing".into()))?;
            let results: Vec<_> = next
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|message| {
                    message
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .filter(|item| item.get("type") == Some(&json!("tool_result")))
                .collect();
            for id in ["history-call", "producer-call"] {
                let item = results
                    .iter()
                    .find(|item| item.get("tool_use_id") == Some(&json!(id)))
                    .ok_or_else(|| {
                        ReactError::Other(format!("native tool result missing: {id}"))
                    })?;
                assert!(
                    item.get("is_error").is_none(),
                    "None与Some(false)显式映普通非错误默认，不发DeepSeek忽略的字段"
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn deepseek_actual_publisher_false_and_unknown_history_reach_second_http() -> Result<()> {
    parent(
        "deepseek_actual_publisher_false_and_unknown_history_reach_second_http",
        false,
    )
    .await
}
#[tokio::test]
async fn deepseek_actual_publisher_true_rejects_before_second_native_http() -> Result<()> {
    parent(
        "deepseek_actual_publisher_true_rejects_before_second_native_http",
        true,
    )
    .await
}
