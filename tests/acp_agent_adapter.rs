#![cfg(feature = "acp")]

use agent_client_protocol::schema::{ProtocolVersion, v1};
use agent_client_protocol::{
    Agent as AcpRole, Channel, Client, ConnectTo as _, ConnectionTo, Error, ErrorCode,
};
use echo_agent::acp::{
    AcpAdapterConfig, AcpAgentAdapter, AcpConnectionProfile, AcpSessionContext, AcpSessionFactory,
};
use echo_agent::agent::{
    Agent, AgentEvent, AgentInvocationContext, AgentPhase, AssistantMessageBlock, AssistantPhase,
    CancellationToken, ExecutionStatus, ExecutionStatusKind, ToolInvocation,
};
use echo_agent::error::AgentFailure;
use echo_agent::error::{ReactError, Result};
use echo_agent::llm::types::{ContentPart, Message, MessageContent};
use echo_agent::tools::{ToolFailure, ToolFailureCategory, ToolResult, ToolStreamEvent};
use futures::FutureExt as _;
use futures::future::{BoxFuture, Shared};
use futures::stream::{self, BoxStream, StreamExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::Notify;

type OriginalProducerJoin = Shared<BoxFuture<'static, std::result::Result<(), String>>>;

struct RecordingAgent {
    id: usize,
    turns: AtomicUsize,
    closed: Arc<AtomicBool>,
    gate: Arc<TurnGate>,
}

impl RecordingAgent {
    fn new(id: usize, closed: Arc<AtomicBool>, gate: Arc<TurnGate>) -> Self {
        Self {
            id,
            turns: AtomicUsize::new(0),
            closed,
            gate,
        }
    }
}

#[derive(Default)]
struct TurnGate {
    started: AtomicUsize,
    cancelled: AtomicUsize,
    close_calls: AtomicUsize,
    producer_join: Mutex<Option<OriginalProducerJoin>>,
    started_notify: Notify,
    cancelled_notify: Notify,
    release: Notify,
}

impl TurnGate {
    async fn wait_started(&self, expected: usize) -> agent_client_protocol::Result<()> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let notified = self.started_notify.notified();
                if self.started.load(Ordering::Acquire) >= expected {
                    return;
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| client_error("timed out waiting for concurrent ACP Prompts"))
    }

    async fn wait_cancelled(&self) -> agent_client_protocol::Result<()> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let notified = self.cancelled_notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.cancelled.load(Ordering::Acquire) > 0 {
                    return;
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| client_error("original producer did not observe cancellation"))
    }
}

impl Drop for RecordingAgent {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

// 当前 fixture 是 typed producer，自持原 invocation 和资源，不再把旧 text 流包装成原生成功。
fn fixture_execution_id(invocation: &AgentInvocationContext) -> Result<String> {
    invocation
        .runtime
        .as_ref()
        .and_then(|runtime| {
            runtime
                .execution_id
                .clone()
                .or_else(|| runtime.turn_id.clone())
        })
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            ReactError::Other("fixture requires its admitted invocation identity".to_string())
        })
}

fn fixture_status(
    execution: &str,
    request: &str,
    sequence: u64,
    status: ExecutionStatusKind,
) -> AgentEvent {
    let phase = if matches!(
        status,
        ExecutionStatusKind::Settled | ExecutionStatusKind::Failed | ExecutionStatusKind::Cancelled
    ) {
        AgentPhase::Terminal
    } else {
        AgentPhase::Thinking
    };
    AgentEvent::ExecutionStatus(ExecutionStatus {
        adapter_generated: false,
        execution_id: execution.to_string(),
        request_id: Some(request.to_string()),
        sequence,
        phase,
        status,
        reason: None,
    })
}

fn fixture_block(
    request: &str,
    id: &str,
    sequence: u64,
    phase: AssistantPhase,
    text: &str,
) -> AgentEvent {
    AgentEvent::AssistantMessage {
        block: AssistantMessageBlock {
            provider_source: None,
            block_id: id.to_string(),
            request_id: request.to_string(),
            sequence,
            phase,
            delta: text.to_string(),
            complete: true,
            provider: None,
            reasoning: None,
        },
    }
}

impl RecordingAgent {
    fn source<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let id = self.id;
        let turn = self.turns.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        Box::pin(async move {
            let plain = match &message {
                Message {
                    role: echo_agent::llm::types::Role::User,
                    content: MessageContent::Text(text),
                    tool_calls: None,
                    name: None,
                    tool_call_id: None,
                    reasoning_content: None,
                    reasoning_blocks: None,
                    assistant_blocks: None,
                    execution_status: None,
                    approved_assistant_blocks: None,
                    tool_result_is_error: None,
                    provider_response: None,
                } => Some(text.clone()),
                _ => None,
            };
            let task = plain.clone().unwrap_or_default();
            if task == "not-settled-setup" {
                return Err(ReactError::ExecutionNotSettled {
                    run_id: "original-foreign-run".to_string(),
                    execution_id: Some("original-foreign-execution".to_string()),
                });
            }
            if task == "ordinary-setup-error" {
                return Err(ReactError::Other("ordinary setup failure".to_string()));
            }
            let execution = fixture_execution_id(&invocation)?;
            let request = format!("{execution}:request");
            let events = async_stream::stream! {
                // 保原输入、能力边界及 guards 至这一条原 source 的真实 EOF。
                let _original_message = message.clone();
                let _original_invocation = invocation;
                yield Ok(fixture_status(&execution, &request, 0, ExecutionStatusKind::Started));
                if task == "held-receipt-after-actor-error" {
                    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
                    let gate = self.gate.clone();
                    let producer_execution = execution.clone();
                    let producer_request = request.clone();
                    let producer = tokio::spawn(async move {
                        cancel.cancelled().await;
                        gate.cancelled.fetch_add(1, Ordering::AcqRel);
                        gate.cancelled_notify.notify_waiters();
                        gate.release.notified().await;
                        let _ = sender.send(Ok(fixture_status(&producer_execution, &producer_request, 1, ExecutionStatusKind::Cancelled))).await;
                    });
                    // 两个观察者仍等待同一个原 JoinHandle；连接关闭不能替换或重启它。
                    let producer_join = async move { producer.await.map_err(|error| error.to_string()) }.boxed().shared();
                    *self.gate.producer_join.lock().unwrap_or_else(|error| error.into_inner()) = Some(producer_join.clone());
                    self.gate.started.fetch_add(1, Ordering::AcqRel);
                    self.gate.started_notify.notify_waiters();
                    let terminal = receiver.recv().await;
                    match producer_join.await {
                        Ok(()) => { if let Some(terminal) = terminal { yield terminal; } }
                        Err(error) => yield Err(ReactError::Other(error)),
                    }
                    return;
                }
                if task == "not-settled-after-cancel" {
                    self.gate.started.fetch_add(1, Ordering::AcqRel);
                    self.gate.started_notify.notify_waiters();
                    cancel.cancelled().await;
                    yield Err(ReactError::ExecutionNotSettled {
                        run_id: "original-foreign-run".to_string(), execution_id: Some("original-foreign-execution".to_string()),
                    });
                    self.gate.started.fetch_add(1, Ordering::AcqRel);
                    self.gate.started_notify.notify_waiters();
                    self.gate.release.notified().await;
                    self.gate.cancelled.fetch_add(1, Ordering::AcqRel);
                    return;
                }
                if task == "wait-for-cancel" {
                    cancel.cancelled().await;
                    yield Ok(fixture_status(&execution, &request, 1, ExecutionStatusKind::Cancelled));
                    return;
                }
                if task == "wait-for-release" {
                    self.gate.started.fetch_add(1, Ordering::AcqRel);
                    self.gate.started_notify.notify_waiters();
                    let released = tokio::select! {
                        () = self.gate.release.notified() => true,
                        () = cancel.cancelled() => false,
                    };
                    if released {
                        yield Ok(AgentEvent::FinalAnswer(format!("agent-{id}-released")));
                        yield Ok(fixture_status(&execution, &request, 1, ExecutionStatusKind::Settled));
                    } else {
                        self.gate.cancelled.fetch_add(1, Ordering::AcqRel);
                        yield Ok(fixture_status(&execution, &request, 1, ExecutionStatusKind::Cancelled));
                    }
                    return;
                }
                if task == "fail" {
                    let failure = AgentFailure::message("acp-test", "scripted framework failure");
                    yield Ok(AgentEvent::Error { source: "acp-test".to_string(), message: failure.message.clone(), failure });
                    yield Ok(fixture_status(&execution, &request, 1, ExecutionStatusKind::Failed));
                    return;
                }
                if task == "tool" || task == "tool-failure" {
                    let failed = task == "tool-failure";
                    let call_id = if failed { "call-failed" } else { "call-1" };
                    let args = if failed { serde_json::json!({}) } else { serde_json::json!({"path":"README.md"}) };
                    yield Ok(AgentEvent::ToolCall { call_id: call_id.to_string(), invocation: ToolInvocation {
                        requested_name: "inspect".to_string(), requested_args: args.clone(),
                        name: "inspect".to_string(), args, rewrites: Vec::new(),
                    } });
                    if !failed {
                        yield Ok(AgentEvent::ToolStream { call_id: call_id.to_string(), name: "inspect".to_string(),
                            event: ToolStreamEvent::Progress { message: "reading".to_string(), percent: Some(50) } });
                    }
                    let mut result = ToolResult::success(if failed { "structured failure without error text" } else { "done" });
                    if failed {
                        result.success = false;
                        result.error = None;
                        result.failure = Some(ToolFailure::new(ToolFailureCategory::Permanent));
                    }
                    yield Ok(AgentEvent::ToolResult { call_id: call_id.to_string(), name: "inspect".to_string(), result });
                    yield Ok(AgentEvent::FinalAnswer(if failed { "tool-failed" } else { "tool-done" }.to_string()));
                    yield Ok(fixture_status(&execution, &request, 1, ExecutionStatusKind::Settled));
                    return;
                }
                if task == "many-small-updates" {
                    for (sequence, text) in [(0, "a"), (1, "b"), (2, "c")] {
                        let mut event = fixture_block(&request, "answer", sequence, AssistantPhase::Unknown, text);
                        if let AgentEvent::AssistantMessage { block } = &mut event { block.complete = sequence == 2; }
                        yield Ok(event);
                    }
                    yield Ok(AgentEvent::FinalAnswer("abc".to_string()));
                    yield Ok(fixture_status(&execution, &request, 1, ExecutionStatusKind::Settled));
                    return;
                }
                let answer = if let Some(text) = plain {
                    yield Ok(fixture_block(&request, "reasoning", 0, AssistantPhase::Reasoning, "reasoning"));
                    format!("agent-{id}-turn-{turn}:{text}")
                } else {
                    let rendered = match &message.content {
                        MessageContent::Parts(parts) => parts.iter().map(|part| match part {
                            ContentPart::Text { text } => format!("text:{text}"),
                            ContentPart::ResourceLink { resource } => format!("resource:{}", serde_json::to_string(resource).unwrap_or_default()),
                            ContentPart::ImageUrl { .. } => "image".to_string(),
                            ContentPart::File { name, .. } => format!("file:{name}"),
                        }).collect::<Vec<_>>().join("|"),
                        content => content.as_text().unwrap_or_default(),
                    };
                    format!("agent-{id}-turn-{turn}-structured:{rendered}")
                };
                yield Ok(fixture_block(&request, "answer", 1, AssistantPhase::FinalAnswer, &answer));
                yield Ok(AgentEvent::FinalAnswer(answer));
                yield Ok(fixture_status(&execution, &request, 1, ExecutionStatusKind::Settled));
            };
            Ok(Box::pin(events) as BoxStream<'a, Result<AgentEvent>>)
        })
    }
}

impl Agent for RecordingAgent {
    fn name(&self) -> &str {
        "recording-acp-agent"
    }
    fn model_name(&self) -> &str {
        "deterministic"
    }
    fn system_prompt(&self) -> &str {
        "ACP adapter test"
    }
    fn close<'a>(&'a self) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.gate.close_calls.fetch_add(1, Ordering::AcqRel);
            self.closed.store(true, Ordering::Release);
            Ok(())
        })
    }
    fn chat_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.source(message, cancel, invocation)
    }
    fn execute_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.source(message, cancel, invocation)
    }
}

struct InvocationGuardDrop(Arc<AtomicUsize>);

impl Drop for InvocationGuardDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

#[tokio::test]
async fn typed_recording_fixture_retains_original_identity_and_guards_until_eof() -> Result<()> {
    let drops = Arc::new(AtomicUsize::new(0));
    let agent = RecordingAgent::new(
        7,
        Arc::new(AtomicBool::new(false)),
        Arc::new(TurnGate::default()),
    );
    let invocation = AgentInvocationContext {
        runtime: Some(echo_agent::tools::ExternalRunContext {
            execution_id: Some("original-invocation".to_string()),
            turn_id: Some("reserved-turn".to_string()),
            resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                InvocationGuardDrop(drops.clone()),
            )],
            ..Default::default()
        }),
        resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
            InvocationGuardDrop(drops.clone()),
        )],
        ..Default::default()
    };
    let mut source = agent
        .chat_stream_message_with_invocation_context(
            Message::user("original 中文".to_string()),
            CancellationToken::new(),
            invocation,
        )
        .await?;
    let mut terminal_seen = false;
    while let Some(event) = source.next().await {
        if let AgentEvent::ExecutionStatus(status) = event? {
            assert_eq!(status.execution_id, "original-invocation");
            assert!(!status.adapter_generated);
            if status.status == ExecutionStatusKind::Settled {
                terminal_seen = true;
                break;
            }
        }
    }
    assert!(terminal_seen);
    assert_eq!(
        drops.load(Ordering::Acquire),
        0,
        "显示terminal不能提前释放原invocation资源"
    );
    assert!(source.next().await.is_none());
    assert_eq!(
        drops.load(Ordering::Acquire),
        2,
        "同一原source的EOF释放完整invocation"
    );
    Ok(())
}

// 用于投影/receipt/close边界测试的当前 typed producer；不提供旧 text fallback。
struct NativeEchoAgent;

fn native_echo_source<'a>(
    message: Message,
    invocation: AgentInvocationContext,
) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
    Box::pin(async move {
        let execution = fixture_execution_id(&invocation)?;
        let request = format!("{execution}:request");
        let answer = message.content.as_text().ok_or_else(|| {
            ReactError::Other("native echo fixture requires text input".to_string())
        })?;
        let source = async_stream::stream! {
            let _original_message = message;
            let _original_invocation = invocation;
            yield Ok(fixture_status(&execution, &request, 0, ExecutionStatusKind::Started));
            yield Ok(AgentEvent::FinalAnswer(answer));
            yield Ok(fixture_status(&execution, &request, 1, ExecutionStatusKind::Settled));
        };
        Ok(Box::pin(source) as BoxStream<'a, Result<AgentEvent>>)
    })
}

impl Agent for NativeEchoAgent {
    fn name(&self) -> &str {
        "native-echo"
    }
    fn model_name(&self) -> &str {
        "test"
    }
    fn system_prompt(&self) -> &str {
        "test"
    }
    fn chat_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        _cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        native_echo_source(message, invocation)
    }
    fn execute_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        _cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        native_echo_source(message, invocation)
    }
}

struct TextOnlyAgent;

impl Agent for TextOnlyAgent {
    fn name(&self) -> &str {
        "text-only"
    }

    fn model_name(&self) -> &str {
        "test"
    }

    fn system_prompt(&self) -> &str {
        "test"
    }

    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move { Ok(task.to_string()) })
    }

    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let answer = task.to_string();
        Box::pin(async move { Ok(stream::iter(vec![Ok(AgentEvent::FinalAnswer(answer))]).boxed()) })
    }

    fn chat_stream<'a>(
        &'a self,
        message: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.execute_stream(message)
    }
}

struct FailingSettlementProfile;

struct FailingFlushProfile {
    flushes: Arc<AtomicUsize>,
}

impl AcpConnectionProfile for FailingFlushProfile {
    fn negotiate_hello(&self, _hello: &serde_json::Value) -> std::result::Result<(), String> {
        Err("standard profile".to_string())
    }

    fn flush_before_agents(&self) -> std::result::Result<(), String> {
        self.flushes.fetch_add(1, Ordering::AcqRel);
        Err("profile flush failed".to_string())
    }

    fn attach(
        &self,
        _services: Arc<echo_agent::acp::AcpConnectionServices>,
    ) -> agent_client_protocol::Builder<
        AcpRole,
        impl agent_client_protocol::HandleDispatchFrom<Client>,
        impl agent_client_protocol::RunWithConnectionTo<Client>,
        impl agent_client_protocol::HandleConnectionClose<Client>,
        agent_client_protocol::RawConnectionContext,
    > {
        AcpRole.builder()
    }
}

struct CountingCloseAgent {
    closes: Arc<AtomicUsize>,
    failures_before_success: usize,
}

impl Agent for CountingCloseAgent {
    fn name(&self) -> &str {
        "counting-close"
    }
    fn model_name(&self) -> &str {
        "test"
    }
    fn system_prompt(&self) -> &str {
        "test"
    }
    fn chat_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        _cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        native_echo_source(message, invocation)
    }
    fn close(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let attempt = self.closes.fetch_add(1, Ordering::AcqRel);
            if attempt < self.failures_before_success {
                Err(ReactError::Other("Agent close failed".to_string()))
            } else {
                Ok(())
            }
        })
    }
}

impl AcpConnectionProfile for FailingSettlementProfile {
    fn negotiate_hello(&self, _hello: &serde_json::Value) -> std::result::Result<(), String> {
        Err("no extension negotiation".to_string())
    }

    fn persist_run_settled(
        &self,
        _entry: &echo_agent::acp::RunEntry,
        _receipt: &echo_agent::runtime::TurnReceipt,
    ) -> std::result::Result<(), String> {
        Err("injected receipt persistence failure".to_string())
    }

    fn attach(
        &self,
        _services: Arc<echo_agent::acp::AcpConnectionServices>,
    ) -> agent_client_protocol::Builder<
        AcpRole,
        impl agent_client_protocol::HandleDispatchFrom<Client>,
        impl agent_client_protocol::RunWithConnectionTo<Client>,
        impl agent_client_protocol::HandleConnectionClose<Client>,
        agent_client_protocol::RawConnectionContext,
    > {
        AcpRole.builder()
    }
}

#[derive(Clone)]
struct TestFactory {
    next_id: Arc<AtomicUsize>,
    contexts: Arc<Mutex<Vec<AcpSessionContext>>>,
    closed: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
    gate: Arc<TurnGate>,
    close_owners: Arc<Mutex<Vec<echo_agent::acp::AcpAdapterCloseHandle>>>,
}

impl TestFactory {
    fn new() -> Self {
        Self {
            next_id: Arc::new(AtomicUsize::new(0)),
            contexts: Arc::new(Mutex::new(Vec::new())),
            closed: Arc::new(Mutex::new(Vec::new())),
            gate: Arc::new(TurnGate::default()),
            close_owners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn session_factory(&self) -> impl AcpSessionFactory {
        let next_id = Arc::clone(&self.next_id);
        let contexts = Arc::clone(&self.contexts);
        let closed = Arc::clone(&self.closed);
        let gate = Arc::clone(&self.gate);
        move |context: AcpSessionContext| {
            let next_id = Arc::clone(&next_id);
            let contexts = Arc::clone(&contexts);
            let closed = Arc::clone(&closed);
            let gate = Arc::clone(&gate);
            async move {
                let id = next_id.fetch_add(1, Ordering::AcqRel);
                let closed_flag = Arc::new(AtomicBool::new(false));
                contexts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(context);
                closed
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(Arc::clone(&closed_flag));
                Ok(Box::new(RecordingAgent::new(id, closed_flag, gate)) as Box<dyn Agent>)
            }
        }
    }

    fn adapter(&self) -> AcpAgentAdapter {
        let adapter = AcpAgentAdapter::new(self.session_factory());
        self.close_owners
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(adapter.close_owner());
        adapter
    }

    fn adapter_with_config(&self, config: AcpAdapterConfig) -> Result<AcpAgentAdapter> {
        let adapter = AcpAgentAdapter::with_config(self.session_factory(), config)?;
        self.close_owners
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(adapter.close_owner());
        Ok(adapter)
    }

    async fn close_retained(&self) -> agent_client_protocol::Result<()> {
        let owners = self
            .close_owners
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        for owner in owners {
            owner.close().await?;
        }
        Ok(())
    }
}

fn absolute_test_path(name: &str) -> Result<PathBuf> {
    std::env::current_dir()
        .map(|path| path.join(name))
        .map_err(|error| ReactError::Other(format!("failed to resolve test path: {error}")))
}

fn client_error(message: &str) -> Error {
    Error::internal_error().data(message.to_string())
}

#[derive(Clone, Default)]
struct CapturedRunRuntime {
    services: Arc<OnceLock<Arc<echo_agent::acp::AcpConnectionServices>>>,
    connection: Arc<OnceLock<ConnectionTo<Client>>>,
}

impl AcpConnectionProfile for CapturedRunRuntime {
    fn negotiate_hello(&self, _hello: &serde_json::Value) -> std::result::Result<(), String> {
        Err("standard-only test profile".to_string())
    }

    fn attach(
        &self,
        services: Arc<echo_agent::acp::AcpConnectionServices>,
    ) -> agent_client_protocol::Builder<
        AcpRole,
        impl agent_client_protocol::HandleDispatchFrom<Client>,
        impl agent_client_protocol::RunWithConnectionTo<Client>,
        impl agent_client_protocol::HandleConnectionClose<Client>,
        agent_client_protocol::RawConnectionContext,
    > {
        let _ = self.services.set(services);
        let captured = self.connection.clone();
        AcpRole.builder().with_spawned(async move |connection| {
            let _ = captured.set(connection);
            Ok(())
        })
    }
}

async fn run_error_isolation_case(
    shared_start: bool,
    cancelled_stream: bool,
) -> agent_client_protocol::Result<()> {
    let factory = TestFactory::new();
    let gate = factory.gate.clone();
    let captured = CapturedRunRuntime::default();
    let client_runtime = captured.clone();
    let adapter = AcpAgentAdapter::with_config(
        factory.session_factory(),
        AcpAdapterConfig {
            shutdown_timeout: Duration::from_millis(50),
            ..AcpAdapterConfig::default()
        },
    )
    .map_err(Error::into_internal_error)?
    .with_profile(captured);
    let _close_owner = adapter.close_owner();
    let (server, client) = Channel::duplex();
    let (server_result, client_result) = tokio::join!(
        adapter.connect_to(server),
        Client
            .builder()
            .connect_with(client, async move |connection| {
                connection
                    .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session_id = connection
                    .send_request(v1::NewSessionRequest::new(
                        absolute_test_path("run-error-isolation")
                            .map_err(Error::into_internal_error)?,
                    ))
                    .block_task()
                    .await?
                    .session_id;
                let services = client_runtime
                    .services
                    .get()
                    .cloned()
                    .ok_or_else(|| client_error("server services missing"))?;
                let session = services
                    .sessions()
                    .get(&session_id)
                    .await
                    .ok_or_else(|| client_error("original Session missing"))?;
                let entry = if shared_start {
                    // This is the same framework entry point and official spawn used by SDK Host.
                    let mut spec =
                        extension_spec(&session, None).map_err(Error::into_internal_error)?;
                    spec.turn.input =
                        echo_agent::runtime::TurnInput::Text("not-settled-setup".to_string());
                    let (entry, task) = services
                        .prepare_run(spec)
                        .await
                        .map_err(Error::into_internal_error)?;
                    let server_connection = client_runtime
                        .connection
                        .get()
                        .ok_or_else(|| client_error("server connection missing"))?;
                    let (finished, observed) = tokio::sync::oneshot::channel();
                    server_connection.spawn(async move {
                        let result = task.await;
                        let _ = finished.send(());
                        result
                    })?;
                    observed.await.map_err(Error::into_internal_error)?;
                    entry
                } else {
                    let prompt = connection.send_request(v1::PromptRequest::new(
                        session_id.clone(),
                        vec![v1::ContentBlock::Text(v1::TextContent::new(
                            if cancelled_stream {
                                "not-settled-after-cancel"
                            } else {
                                "not-settled-setup"
                            },
                        ))],
                    ));
                    if cancelled_stream {
                        gate.wait_started(1).await?;
                        prompt.cancel()?;
                        gate.wait_started(2).await?;
                    }
                    let mut response = Box::pin(prompt.block_task());
                    if cancelled_stream {
                        assert!(
                            tokio::time::timeout(Duration::from_millis(10), &mut response)
                                .await
                                .is_err(),
                            "observation cannot drop the held original producer"
                        );
                        let run_id = format!("{}:turn:1", session_id);
                        let original = services
                            .run(&run_id)
                            .await
                            .ok_or_else(|| client_error("held Run missing"))?;
                        assert!(original.receipt().is_none());
                        assert!(session.begin_turn().is_err());
                        // Dispatch remains available even while the original source drains.
                        connection
                            .send_request(v1::NewSessionRequest::new(
                                absolute_test_path("held-other-session")
                                    .map_err(Error::into_internal_error)?,
                            ))
                            .block_task()
                            .await?;
                        gate.release.notify_one();
                    }
                    let error = response
                        .await
                        .err()
                        .ok_or_else(|| client_error("unknown Run returned a successful Prompt"))?;
                    assert_eq!(error.code, ErrorCode::InternalError);
                    assert_eq!(
                        error.data,
                        Some(serde_json::json!({
                            "observation": "not_settled", "run_id": "original-foreign-run",
                            "execution_id": "original-foreign-execution",
                        }))
                    );
                    let run_id = format!("{}:turn:1", session_id);
                    services
                        .run(&run_id)
                        .await
                        .ok_or_else(|| client_error("original Run missing"))?
                };
                assert!(entry.is_running());
                assert!(entry.receipt().is_none());
                assert!(session.begin_turn().is_err());
                assert!(
                    tokio::time::timeout(Duration::from_millis(10), entry.wait_receipt())
                        .await
                        .is_err()
                );
                // An unrelated legal request on the same official connection must still work.
                let other = connection
                    .send_request(v1::NewSessionRequest::new(
                        absolute_test_path("other-session").map_err(Error::into_internal_error)?,
                    ))
                    .block_task()
                    .await?
                    .session_id;
                let ordinary = connection
                    .send_request(v1::PromptRequest::new(
                        other.clone(),
                        vec![v1::ContentBlock::Text(v1::TextContent::new(
                            "ordinary-setup-error",
                        ))],
                    ))
                    .block_task()
                    .await;
                assert_eq!(
                    ordinary.err().map(|error| error.code),
                    Some(ErrorCode::InternalError)
                );
                if cancelled_stream {
                    assert_eq!(
                        gate.cancelled.load(Ordering::Acquire),
                        1,
                        "the same producer reached real EOF"
                    );
                }
                let response = connection
                    .send_request(v1::PromptRequest::new(
                        other,
                        vec![v1::ContentBlock::Text(v1::TextContent::new("healthy"))],
                    ))
                    .block_task()
                    .await?;
                assert_eq!(response.stop_reason, v1::StopReason::EndTurn);
                assert!(services.cancel_run(&entry.run_id).await);
                let _ = services.remove_run(&entry.run_id).await;
                assert!(entry.receipt().is_none());
                assert!(
                    session.begin_turn().is_err(),
                    "cancellation/revocation is not physical closure"
                );
                let recovered = services
                    .run(&entry.run_id)
                    .await
                    .ok_or_else(|| client_error("unknown original Run was removed"))?;
                assert!(Arc::ptr_eq(&entry, &recovered));
                Ok(())
            })
    );
    client_result?;
    assert!(
        server_result.is_err(),
        "EOF must still report the original unsettled owner"
    );
    Ok(())
}

#[tokio::test]
async fn standard_run_error_isolation_keeps_connection_and_original_owner()
-> agent_client_protocol::Result<()> {
    run_error_isolation_case(false, false).await
}

#[tokio::test]
async fn shared_run_error_isolation_keeps_connection_and_original_owner()
-> agent_client_protocol::Result<()> {
    run_error_isolation_case(true, false).await
}

#[tokio::test]
async fn cancelled_stream_run_error_isolation_drains_original_source_before_responding()
-> agent_client_protocol::Result<()> {
    run_error_isolation_case(false, true).await
}

async fn actor_failure_keeps_original_driver_case(
    shared_start: bool,
) -> agent_client_protocol::Result<()> {
    let factory = TestFactory::new();
    let gate = factory.gate.clone();
    let captured = CapturedRunRuntime::default();
    let client_runtime = captured.clone();
    let observed_entry = Arc::new(OnceLock::new());
    let client_entry = observed_entry.clone();
    let actor_error_polled = Arc::new(AtomicBool::new(false));
    let client_actor_error = actor_error_polled.clone();
    let connection_done = Arc::new(Notify::new());
    let client_connection_done = connection_done.clone();
    let adapter = AcpAgentAdapter::with_config(
        factory.session_factory(),
        AcpAdapterConfig {
            shutdown_timeout: Duration::from_millis(50),
            ..AcpAdapterConfig::default()
        },
    )
    .map_err(Error::into_internal_error)?
    .with_profile(captured.clone());
    let (server, client) = Channel::duplex();
    let (close_owner, server_future) = adapter.connect_retaining_close_owner(server);
    let client_gate = gate.clone();
    let client_future = Client
        .builder()
        .connect_with(client, async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session_id = connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("actor-failure-held-receipt")
                        .map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?
                .session_id;
            let services = client_runtime
                .services
                .get()
                .cloned()
                .ok_or_else(|| client_error("server services missing"))?;
            let session = services
                .sessions()
                .get(&session_id)
                .await
                .ok_or_else(|| client_error("original Session missing"))?;
            let server_connection = client_runtime
                .connection
                .get()
                .ok_or_else(|| client_error("server connection missing"))?;
            let pending_prompt = if shared_start {
                // Same entry and official spawn used by the independent SDK profile.
                let mut spec =
                    extension_spec(&session, None).map_err(Error::into_internal_error)?;
                spec.turn.input = echo_agent::runtime::TurnInput::Text(
                    "held-receipt-after-actor-error".to_string(),
                );
                let (entry, task) = services
                    .prepare_run(spec)
                    .await
                    .map_err(Error::into_internal_error)?;
                let _ = client_entry.set(entry);
                server_connection.spawn(task)?;
                None
            } else {
                Some(connection.send_request(v1::PromptRequest::new(
                    session_id.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new(
                        "held-receipt-after-actor-error",
                    ))],
                )))
            };
            client_gate.wait_started(1).await?;
            let run_id = format!("{session_id}:turn:1");
            let entry = services
                .run(&run_id)
                .await
                .ok_or_else(|| client_error("started original Run missing"))?;
            assert!(entry.receipt().is_none());
            assert!(session.begin_turn().is_err());
            let _ = client_entry.set(entry);
            // The original raw stream was polled before this unrelated task
            // fails. Do not use EOF or elapsed time to trigger the failure.
            server_connection.spawn(async move {
                client_actor_error.store(true, Ordering::Release);
                Err(client_error("injected other ACP task failure"))
            })?;
            client_connection_done.notified().await;
            drop(pending_prompt);
            Ok(())
        });
    let server_future = async move {
        let result = server_future.await;
        connection_done.notify_one();
        result
    };
    let (server_result, _client_result) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(server_future, client_future)
    })
    .await
    .map_err(|_| client_error("controlled ACP actor failure did not return"))?;
    assert!(actor_error_polled.load(Ordering::Acquire));
    assert!(server_result.is_err());
    let entry = observed_entry
        .get()
        .cloned()
        .ok_or_else(|| client_error("original Run was not captured"))?;
    let services = captured
        .services
        .get()
        .ok_or_else(|| client_error("retained services missing"))?;
    let session = services
        .sessions()
        .get(&entry.session_id)
        .await
        .ok_or_else(|| client_error("retained Session missing"))?;
    assert!(entry.receipt().is_none(), "held producer has no receipt");
    gate.wait_cancelled().await?;
    assert_eq!(gate.started.load(Ordering::Acquire), 1);
    assert_eq!(gate.cancelled.load(Ordering::Acquire), 1);
    assert_eq!(gate.close_calls.load(Ordering::Acquire), 0);
    let session_admission_retained = session.begin_turn().is_err();
    let original_slot_held = session.wait_until_idle().now_or_never().is_none();
    let producer_join = gate
        .producer_join
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .ok_or_else(|| client_error("original producer JoinHandle missing"))?;
    gate.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), producer_join)
        .await
        .map_err(|_| client_error("original producer did not finish after release"))?
        .map_err(|error| client_error(&error))?;
    let receipt = tokio::time::timeout(Duration::from_secs(2), entry.wait_receipt())
        .await
        .map_err(|_| client_error("original driver lost before publishing its real receipt"))?;
    assert!(session_admission_retained);
    assert!(
        original_slot_held,
        "original active slot must survive actor loss"
    );
    assert_eq!(receipt.turn_id.as_str(), entry.run_id);
    assert_eq!(receipt.outcome, echo_agent::runtime::TurnOutcome::Cancelled);
    let published = entry
        .receipt()
        .ok_or_else(|| client_error("original Run did not retain its receipt"))?;
    assert!(Arc::ptr_eq(&receipt, &published));
    assert_eq!(gate.started.load(Ordering::Acquire), 1);
    assert_eq!(gate.cancelled.load(Ordering::Acquire), 1);
    close_owner.close().await?;
    assert_eq!(gate.close_calls.load(Ordering::Acquire), 1);
    close_owner.close().await?;
    assert_eq!(gate.close_calls.load(Ordering::Acquire), 1);
    Ok(())
}

#[tokio::test]
async fn standard_actor_failure_keeps_original_driver_until_receipt()
-> agent_client_protocol::Result<()> {
    actor_failure_keeps_original_driver_case(false).await
}

#[tokio::test]
async fn shared_actor_failure_keeps_original_driver_until_receipt()
-> agent_client_protocol::Result<()> {
    actor_failure_keeps_original_driver_case(true).await
}

#[test]
fn acp_fixtures_use_official_typed_messages() -> agent_client_protocol::Result<()> {
    let prompt: v1::PromptRequest = serde_json::from_str(include_str!(
        "fixtures/acp/v1/prompt-resource-link-valid.json"
    ))
    .map_err(Error::into_internal_error)?;
    assert_eq!(prompt.session_id, v1::SessionId::new("sess_fixture"));
    assert_eq!(prompt.prompt.len(), 2);

    let invalid_session: v1::NewSessionRequest = serde_json::from_str(include_str!(
        "fixtures/acp/v1/session-relative-cwd-invalid.json"
    ))
    .map_err(Error::into_internal_error)?;
    assert!(!invalid_session.cwd.is_absolute());
    Ok(())
}

#[test]
fn adapter_config_rejects_zero_resource_bounds() {
    let config = AcpAdapterConfig {
        max_sessions: 0,
        ..AcpAdapterConfig::default()
    };
    assert!(config.validate().is_err());

    let config = AcpAdapterConfig {
        shutdown_timeout: Duration::ZERO,
        ..AcpAdapterConfig::default()
    };
    assert!(config.validate().is_err());
}

#[tokio::test]
async fn oversized_projected_update_fails_the_prompt_without_closing_the_connection()
-> agent_client_protocol::Result<()> {
    let factory = TestFactory::new();
    let adapter = factory
        .adapter_with_config(AcpAdapterConfig {
            max_update_chars: 4,
            ..AcpAdapterConfig::default()
        })
        .map_err(Error::into_internal_error)?;
    Client
        .builder()
        .connect_with(adapter, async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-bounds").map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?
                .session_id;
            let oversized = connection
                .send_request(v1::PromptRequest::new(
                    session.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new("large"))],
                ))
                .block_task()
                .await;
            assert_eq!(
                oversized.err().map(|error| error.code),
                Some(ErrorCode::InternalError)
            );

            let unsupported = connection
                .send_request(v1::PromptRequest::new(
                    session,
                    vec![v1::ContentBlock::Image(v1::ImageContent::new(
                        "aQ==",
                        "image/png",
                    ))],
                ))
                .block_task()
                .await;
            assert_eq!(
                unsupported.err().map(|error| error.code),
                Some(ErrorCode::InvalidParams)
            );
            Ok(())
        })
        .await
}

#[tokio::test]
async fn terminal_projection_failure_never_returns_end_turn() -> agent_client_protocol::Result<()> {
    let adapter = AcpAgentAdapter::with_config(
        |_context: AcpSessionContext| async { Ok(Box::new(NativeEchoAgent) as Box<dyn Agent>) },
        AcpAdapterConfig {
            max_update_chars: 4,
            ..AcpAdapterConfig::default()
        },
    )
    .map_err(Error::into_internal_error)?;
    let _close_owner = adapter.close_owner();
    Client
        .builder()
        .connect_with(adapter, async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-terminal-projection")
                        .map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?
                .session_id;
            let response = connection
                .send_request(v1::PromptRequest::new(
                    session,
                    vec![v1::ContentBlock::Text(v1::TextContent::new("large"))],
                ))
                .block_task()
                .await;
            assert_eq!(
                response.err().map(|error| error.code),
                Some(ErrorCode::InternalError)
            );
            Ok(())
        })
        .await
}

#[tokio::test]
async fn receipt_persistence_failure_never_returns_end_turn() -> agent_client_protocol::Result<()> {
    let adapter = AcpAgentAdapter::new(|_context: AcpSessionContext| async {
        Ok(Box::new(NativeEchoAgent) as Box<dyn Agent>)
    })
    .with_profile(FailingSettlementProfile);
    let _close_owner = adapter.close_owner();
    Client
        .builder()
        .on_receive_notification(
            async move |_notification: v1::SessionNotification,
                        _connection: ConnectionTo<AcpRole>| { Ok(()) },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(adapter, async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-receipt-persistence")
                        .map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?
                .session_id;
            let response = connection
                .send_request(v1::PromptRequest::new(
                    session,
                    vec![v1::ContentBlock::Text(v1::TextContent::new("done"))],
                ))
                .block_task()
                .await;
            assert_eq!(
                response.err().map(|error| error.code),
                Some(ErrorCode::InternalError)
            );
            Ok(())
        })
        .await
}

#[tokio::test]
async fn cumulative_update_budget_stops_many_small_notifications()
-> agent_client_protocol::Result<()> {
    let factory = TestFactory::new();
    let adapter = factory
        .adapter_with_config(AcpAdapterConfig {
            max_update_chars: 1_000,
            max_updates_per_turn: 2,
            max_total_update_chars: 2_000,
            ..AcpAdapterConfig::default()
        })
        .map_err(Error::into_internal_error)?;
    Client
        .builder()
        .connect_with(adapter, async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-cumulative-bounds")
                        .map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?
                .session_id;
            let result = connection
                .send_request(v1::PromptRequest::new(
                    session,
                    vec![v1::ContentBlock::Text(v1::TextContent::new(
                        "many-small-updates",
                    ))],
                ))
                .block_task()
                .await;
            assert_eq!(
                result.err().map(|error| error.code),
                Some(ErrorCode::InternalError)
            );
            Ok(())
        })
        .await
}

#[tokio::test]
async fn resource_link_requires_structured_agent_support() -> agent_client_protocol::Result<()> {
    let adapter = AcpAgentAdapter::new(|_context: AcpSessionContext| async {
        Ok(Box::new(TextOnlyAgent) as Box<dyn Agent>)
    });
    let _close_owner = adapter.close_owner();
    Client
        .builder()
        .connect_with(adapter, async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-text-only").map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?
                .session_id;
            let result = connection
                .send_request(v1::PromptRequest::new(
                    session,
                    vec![v1::ContentBlock::ResourceLink(v1::ResourceLink::new(
                        "source",
                        "file:///workspace/lib.rs",
                    ))],
                ))
                .block_task()
                .await;
            assert_eq!(
                result.err().map(|error| error.code),
                Some(ErrorCode::InternalError)
            );
            Ok(())
        })
        .await
}

#[tokio::test]
async fn official_client_observes_isolated_sessions_and_ordered_updates()
-> agent_client_protocol::Result<()> {
    let factory = TestFactory::new();
    let updates = Arc::new(Mutex::new(Vec::<v1::SessionNotification>::new()));
    let captured_updates = updates.clone();
    let cwd = absolute_test_path("acp-primary").map_err(Error::into_internal_error)?;
    let additional = absolute_test_path("acp-shared").map_err(Error::into_internal_error)?;
    let expected_additional = additional.clone();
    let mcp_command = absolute_test_path("fake-mcp-server")
        .map_err(Error::into_internal_error)?
        .to_string_lossy()
        .into_owned();

    Client
        .builder()
        .on_receive_notification(
            async move |notification: v1::SessionNotification,
                        _connection: ConnectionTo<AcpRole>| {
                captured_updates
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(factory.adapter(), async move |connection| {
            let initialized = connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            assert_eq!(initialized.protocol_version, ProtocolVersion::V1);
            assert_eq!(initialized.agent_capabilities, v1::AgentCapabilities::new());
            assert!(initialized.agent_capabilities.meta.is_none());

            let first = connection
                .send_request(
                    v1::NewSessionRequest::new(cwd.clone())
                        .additional_directories(vec![additional.clone()])
                        .mcp_servers(vec![v1::McpServer::Stdio(v1::McpServerStdio::new(
                            "fixture-mcp",
                            mcp_command,
                        ))]),
                )
                .block_task()
                .await?;
            let second = connection
                .send_request(v1::NewSessionRequest::new(cwd))
                .block_task()
                .await?;
            assert_ne!(first.session_id, second.session_id);

            for (session_id, prompt) in [
                (first.session_id.clone(), "hello"),
                (first.session_id.clone(), "again"),
                (second.session_id.clone(), "separate"),
            ] {
                let response = connection
                    .send_request(v1::PromptRequest::new(
                        session_id,
                        vec![v1::ContentBlock::Text(v1::TextContent::new(prompt))],
                    ))
                    .block_task()
                    .await?;
                assert_eq!(response.stop_reason, v1::StopReason::EndTurn);
            }
            Ok(())
        })
        .await?;
    factory.close_retained().await?;

    let contexts = factory
        .contexts
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(contexts.len(), 2);
    assert_eq!(
        contexts
            .first()
            .map(|context| &context.additional_directories),
        Some(&vec![expected_additional])
    );
    assert_eq!(
        contexts.first().map(|context| context.mcp_servers.len()),
        Some(1)
    );
    assert_eq!(
        contexts.get(1).map(|context| context.mcp_servers.len()),
        Some(0)
    );
    drop(contexts);

    let updates = updates.lock().unwrap_or_else(|error| error.into_inner());
    let text = updates
        .iter()
        .filter_map(|notification| match &notification.update {
            v1::SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                v1::ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        text,
        vec![
            "agent-0-turn-1:hello",
            "agent-0-turn-2:again",
            "agent-1-turn-1:separate",
        ]
    );
    assert_eq!(
        updates
            .iter()
            .filter(|notification| matches!(
                notification.update,
                v1::SessionUpdate::AgentThoughtChunk(_)
            ))
            .count(),
        3
    );
    drop(updates);
    assert!(
        factory
            .closed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .all(|closed| closed.load(Ordering::Acquire))
    );
    Ok(())
}

#[tokio::test]
async fn both_cancellation_routes_settle_prompt_without_blocking_dispatch()
-> agent_client_protocol::Result<()> {
    let factory = TestFactory::new();
    Client
        .builder()
        .connect_with(factory.adapter(), async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-cancel").map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?
                .session_id;

            let cancelled_by_session = connection.send_request(v1::PromptRequest::new(
                session.clone(),
                vec![v1::ContentBlock::Text(v1::TextContent::new(
                    "wait-for-cancel",
                ))],
            ));
            connection.send_notification(v1::CancelNotification::new(session.clone()))?;
            let response =
                tokio::time::timeout(Duration::from_secs(2), cancelled_by_session.block_task())
                    .await
                    .map_err(|_| client_error("session/cancel did not settle the Prompt"))??;
            assert_eq!(response.stop_reason, v1::StopReason::Cancelled);

            let cancelled_by_request = connection.send_request(v1::PromptRequest::new(
                session.clone(),
                vec![v1::ContentBlock::Text(v1::TextContent::new(
                    "wait-for-cancel",
                ))],
            ));
            cancelled_by_request.cancel()?;
            let response =
                tokio::time::timeout(Duration::from_secs(2), cancelled_by_request.block_task())
                    .await
                    .map_err(|_| {
                        client_error("request cancellation did not settle the Prompt")
                    })??;
            assert_eq!(response.stop_reason, v1::StopReason::Cancelled);

            let recovered = connection
                .send_request(v1::PromptRequest::new(
                    session,
                    vec![v1::ContentBlock::Text(v1::TextContent::new("after-cancel"))],
                ))
                .block_task()
                .await?;
            assert_eq!(recovered.stop_reason, v1::StopReason::EndTurn);
            Ok(())
        })
        .await
}

#[tokio::test]
async fn separate_sessions_drive_prompts_concurrently() -> agent_client_protocol::Result<()> {
    let factory = TestFactory::new();
    let gate = factory.gate.clone();
    Client
        .builder()
        .connect_with(factory.adapter(), async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let cwd = absolute_test_path("acp-concurrent").map_err(Error::into_internal_error)?;
            let first_session = connection
                .send_request(v1::NewSessionRequest::new(cwd.clone()))
                .block_task()
                .await?
                .session_id;
            let second_session = connection
                .send_request(v1::NewSessionRequest::new(cwd))
                .block_task()
                .await?
                .session_id;
            let first = connection.send_request(v1::PromptRequest::new(
                first_session,
                vec![v1::ContentBlock::Text(v1::TextContent::new(
                    "wait-for-release",
                ))],
            ));
            let second = connection.send_request(v1::PromptRequest::new(
                second_session,
                vec![v1::ContentBlock::Text(v1::TextContent::new(
                    "wait-for-release",
                ))],
            ));
            gate.wait_started(2).await?;
            gate.release.notify_waiters();
            let (first, second) = tokio::join!(first.block_task(), second.block_task());
            assert_eq!(first?.stop_reason, v1::StopReason::EndTurn);
            assert_eq!(second?.stop_reason, v1::StopReason::EndTurn);
            Ok(())
        })
        .await
}

#[tokio::test]
async fn closing_official_channel_cancels_active_prompt_and_closes_agent()
-> agent_client_protocol::Result<()> {
    let factory = TestFactory::new();
    let gate = factory.gate.clone();
    let adapter = factory
        .adapter_with_config(AcpAdapterConfig {
            shutdown_timeout: Duration::from_secs(1),
            ..AcpAdapterConfig::default()
        })
        .map_err(Error::into_internal_error)?;
    let (agent_transport, client_transport) = Channel::duplex();
    let adapter_future = adapter.connect_to(agent_transport);
    let client_future = Client
        .builder()
        .connect_with(client_transport, async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-channel-close").map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?
                .session_id;
            let pending = connection.send_request(v1::PromptRequest::new(
                session,
                vec![v1::ContentBlock::Text(v1::TextContent::new(
                    "wait-for-release",
                ))],
            ));
            gate.wait_started(1).await?;
            pending.detach();
            Ok(())
        });

    let (adapter_result, client_result) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(adapter_future, client_future)
    })
    .await
    .map_err(|_| client_error("ACP channel close did not settle within its bound"))?;
    if let Err(error) = adapter_result {
        assert_eq!(error.code, ErrorCode::InternalError);
    }
    client_result?;
    assert_eq!(factory.gate.cancelled.load(Ordering::Acquire), 1);
    assert!(
        factory
            .closed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .all(|closed| closed.load(Ordering::Acquire))
    );
    Ok(())
}

#[tokio::test]
async fn profile_flush_failure_still_awaits_agent_close_and_retries_on_eof()
-> agent_client_protocol::Result<()> {
    let closes = Arc::new(AtomicUsize::new(0));
    let flushes = Arc::new(AtomicUsize::new(0));
    let adapter = AcpAgentAdapter::new({
        let closes = Arc::clone(&closes);
        move |_context: AcpSessionContext| {
            let closes = Arc::clone(&closes);
            async move {
                Ok(Box::new(CountingCloseAgent {
                    closes,
                    failures_before_success: 0,
                }) as Box<dyn Agent>)
            }
        }
    })
    .with_profile(FailingFlushProfile {
        flushes: Arc::clone(&flushes),
    });
    let _close_owner = adapter.close_owner();

    let (agent_transport, client_transport) = Channel::duplex();
    let adapter_future = adapter.connect_to(agent_transport);
    let client_future = Client
        .builder()
        .connect_with(client_transport, async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-flush-close").map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?;
            Ok(())
        });
    let (adapter_result, client_result) = tokio::join!(adapter_future, client_future);

    client_result?;
    assert!(adapter_result.is_err());
    assert_eq!(closes.load(Ordering::Acquire), 1);
    assert!(flushes.load(Ordering::Acquire) >= 2);
    Ok(())
}

#[tokio::test]
async fn failed_agent_close_retries_on_connection_eof() -> agent_client_protocol::Result<()> {
    let closes = Arc::new(AtomicUsize::new(0));
    let adapter = AcpAgentAdapter::new({
        let closes = Arc::clone(&closes);
        move |_context: AcpSessionContext| {
            let closes = Arc::clone(&closes);
            async move {
                Ok(Box::new(CountingCloseAgent {
                    closes,
                    failures_before_success: 1,
                }) as Box<dyn Agent>)
            }
        }
    });
    let _close_owner = adapter.close_owner();
    let (agent_transport, client_transport) = Channel::duplex();
    let adapter_future = adapter.connect_to(agent_transport);
    let client_future = Client
        .builder()
        .connect_with(client_transport, async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-agent-close-retry")
                        .map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?;
            Ok(())
        });
    let (adapter_result, client_result) = tokio::join!(adapter_future, client_future);
    client_result?;
    assert!(adapter_result.is_err());
    assert_eq!(closes.load(Ordering::Acquire), 2);
    Ok(())
}

#[tokio::test]
async fn standard_adapter_rejects_connection_without_retained_close_owner()
-> agent_client_protocol::Result<()> {
    let creations = Arc::new(AtomicUsize::new(0));
    let adapter = AcpAgentAdapter::new({
        let creations = Arc::clone(&creations);
        move |_context: AcpSessionContext| {
            let creations = Arc::clone(&creations);
            async move {
                creations.fetch_add(1, Ordering::AcqRel);
                Ok(Box::new(TextOnlyAgent) as Box<dyn Agent>)
            }
        }
    });
    let dropped_owner = adapter.close_owner();
    drop(dropped_owner);
    let (agent_transport, _client_transport) = Channel::duplex();
    let failure = tokio::time::timeout(Duration::from_secs(1), adapter.connect_to(agent_transport))
        .await
        .map_err(|_| client_error("ACP connect without close owner did not reject"))?
        .err()
        .ok_or_else(|| client_error("ACP connect without close owner was accepted"))?;
    assert_eq!(failure.code, ErrorCode::InternalError);
    assert!(failure.to_string().contains("retain a close owner"));
    assert_eq!(creations.load(Ordering::Acquire), 0);
    Ok(())
}

#[tokio::test]
async fn standard_close_owner_retries_after_transport_and_eof_failures()
-> agent_client_protocol::Result<()> {
    let closes = Arc::new(AtomicUsize::new(0));
    let adapter = AcpAgentAdapter::new({
        let closes = Arc::clone(&closes);
        move |_context: AcpSessionContext| {
            let closes = Arc::clone(&closes);
            async move {
                Ok(Box::new(CountingCloseAgent {
                    closes,
                    failures_before_success: 2,
                }) as Box<dyn Agent>)
            }
        }
    });
    let (agent_transport, client_transport) = Channel::duplex();
    let (close_owner, adapter_future) = adapter.connect_retaining_close_owner(agent_transport);
    let client_future = Client
        .builder()
        .connect_with(client_transport, async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-retained-close-owner")
                        .map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?;
            Ok(())
        });
    let (adapter_result, client_result) = tokio::join!(adapter_future, client_future);
    client_result?;
    assert!(adapter_result.is_err());
    assert_eq!(closes.load(Ordering::Acquire), 2);
    close_owner.close().await?;
    assert_eq!(closes.load(Ordering::Acquire), 3);
    close_owner.close().await?;
    assert_eq!(closes.load(Ordering::Acquire), 3);
    Ok(())
}

#[tokio::test]
async fn aborting_connection_future_keeps_preissued_close_owner_after_a_run()
-> agent_client_protocol::Result<()> {
    let closes = Arc::new(AtomicUsize::new(0));
    let adapter = AcpAgentAdapter::new({
        let closes = Arc::clone(&closes);
        move |_context: AcpSessionContext| {
            let closes = Arc::clone(&closes);
            async move {
                Ok(Box::new(CountingCloseAgent {
                    closes,
                    failures_before_success: 0,
                }) as Box<dyn Agent>)
            }
        }
    });
    let (agent_transport, client_transport) = Channel::duplex();
    let (close_owner, connection) = adapter.connect_retaining_close_owner(agent_transport);
    let connected = Arc::new(Notify::new());
    let client_connected = Arc::clone(&connected);
    let adapter_task = tokio::spawn(connection);
    let client_task = tokio::spawn(async move {
        Client
            .builder()
            .connect_with(client_transport, async move |connection| {
                connection
                    .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(v1::NewSessionRequest::new(
                        absolute_test_path("acp-aborted-connection")
                            .map_err(Error::into_internal_error)?,
                    ))
                    .block_task()
                    .await?
                    .session_id;
                connection
                    .send_request(v1::PromptRequest::new(
                        session,
                        vec![v1::ContentBlock::Text(v1::TextContent::new("settled run"))],
                    ))
                    .block_task()
                    .await?;
                client_connected.notify_one();
                std::future::pending::<agent_client_protocol::Result<()>>().await
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), connected.notified())
        .await
        .map_err(|_| client_error("ACP Session/Run did not complete before abort"))?;
    adapter_task.abort();
    let _ = adapter_task.await;
    close_owner.close().await?;
    assert_eq!(closes.load(Ordering::Acquire), 1);
    client_task.abort();
    let _ = client_task.await;
    Ok(())
}

#[tokio::test]
async fn invalid_session_and_concurrent_prompt_are_typed_errors()
-> agent_client_protocol::Result<()> {
    let factory = TestFactory::new();
    Client
        .builder()
        .connect_with(factory.adapter(), async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let relative_session: v1::NewSessionRequest = serde_json::from_str(include_str!(
                "fixtures/acp/v1/session-relative-cwd-invalid.json"
            ))
            .map_err(Error::into_internal_error)?;
            let invalid_cwd = connection.send_request(relative_session).block_task().await;
            assert_eq!(
                invalid_cwd.err().map(|error| error.code),
                Some(ErrorCode::InvalidParams)
            );
            let unknown = connection
                .send_request(v1::PromptRequest::new(
                    v1::SessionId::new("missing"),
                    vec![v1::ContentBlock::Text(v1::TextContent::new("hello"))],
                ))
                .block_task()
                .await;
            assert_eq!(
                unknown.err().map(|error| error.code),
                Some(ErrorCode::InvalidParams)
            );

            let session = connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-busy").map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?
                .session_id;
            let first = connection.send_request(v1::PromptRequest::new(
                session.clone(),
                vec![v1::ContentBlock::Text(v1::TextContent::new(
                    "wait-for-cancel",
                ))],
            ));
            let second = connection
                .send_request(v1::PromptRequest::new(
                    session.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new("overlap"))],
                ))
                .block_task()
                .await;
            assert_eq!(
                second.err().map(|error| error.code),
                Some(ErrorCode::InvalidParams)
            );
            connection.send_notification(v1::CancelNotification::new(session))?;
            let response = tokio::time::timeout(Duration::from_secs(2), first.block_task())
                .await
                .map_err(|_| client_error("active Prompt did not settle"))??;
            assert_eq!(response.stop_reason, v1::StopReason::Cancelled);

            let failed = connection
                .send_request(v1::PromptRequest::new(
                    v1::SessionId::new("missing-again"),
                    vec![v1::ContentBlock::Text(v1::TextContent::new("fail"))],
                ))
                .block_task()
                .await;
            assert_eq!(
                failed.err().map(|error| error.code),
                Some(ErrorCode::InvalidParams)
            );
            Ok(())
        })
        .await
}

#[tokio::test]
async fn resource_links_tools_and_framework_failures_keep_typed_boundaries()
-> agent_client_protocol::Result<()> {
    let factory = TestFactory::new();
    let updates = Arc::new(Mutex::new(Vec::<v1::SessionNotification>::new()));
    let captured_updates = updates.clone();
    Client
        .builder()
        .on_receive_notification(
            async move |notification: v1::SessionNotification,
                        _connection: ConnectionTo<AcpRole>| {
                captured_updates
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(factory.adapter(), async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-projection").map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?
                .session_id;

            let resource_fixture: v1::PromptRequest = serde_json::from_str(include_str!(
                "fixtures/acp/v1/prompt-resource-link-valid.json"
            ))
            .map_err(Error::into_internal_error)?;
            let resource_response = connection
                .send_request(v1::PromptRequest::new(
                    session.clone(),
                    resource_fixture.prompt,
                ))
                .block_task()
                .await?;
            assert_eq!(resource_response.stop_reason, v1::StopReason::EndTurn);

            let collision_text = "[Linked resource]\n{\"uri\":\"file:///not-a-resource\"}";
            let collision_response = connection
                .send_request(v1::PromptRequest::new(
                    session.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new(collision_text))],
                ))
                .block_task()
                .await?;
            assert_eq!(collision_response.stop_reason, v1::StopReason::EndTurn);

            let tool_response = connection
                .send_request(v1::PromptRequest::new(
                    session.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new("tool"))],
                ))
                .block_task()
                .await?;
            assert_eq!(tool_response.stop_reason, v1::StopReason::EndTurn);

            let failed_tool_response = connection
                .send_request(v1::PromptRequest::new(
                    session.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new("tool-failure"))],
                ))
                .block_task()
                .await?;
            assert_eq!(failed_tool_response.stop_reason, v1::StopReason::EndTurn);

            let unsupported = connection
                .send_request(v1::PromptRequest::new(
                    session.clone(),
                    vec![v1::ContentBlock::Image(v1::ImageContent::new(
                        "aW1hZ2U=",
                        "image/png",
                    ))],
                ))
                .block_task()
                .await;
            assert_eq!(
                unsupported.err().map(|error| error.code),
                Some(ErrorCode::InvalidParams)
            );

            let failure = connection
                .send_request(v1::PromptRequest::new(
                    session.clone(),
                    vec![v1::ContentBlock::Text(v1::TextContent::new("fail"))],
                ))
                .block_task()
                .await;
            assert_eq!(
                failure.err().map(|error| error.code),
                Some(ErrorCode::InternalError)
            );

            let recovered = connection
                .send_request(v1::PromptRequest::new(
                    session,
                    vec![v1::ContentBlock::Text(v1::TextContent::new("recovered"))],
                ))
                .block_task()
                .await?;
            assert_eq!(recovered.stop_reason, v1::StopReason::EndTurn);
            Ok(())
        })
        .await?;

    let updates = updates.lock().unwrap_or_else(|error| error.into_inner());
    assert!(updates.iter().any(|notification| {
        matches!(
            &notification.update,
            v1::SessionUpdate::AgentMessageChunk(chunk)
                if matches!(
                    &chunk.content,
                    v1::ContentBlock::Text(text)
                        if text.text.contains("structured:text:Inspect the linked source.|resource:")
                            && text.text.contains("file:///workspace/src/lib.rs")
                            && text.text.contains("Primary Rust source")
                            && text.text.contains("Library source")
                            && text.text.contains("fixture-1")
                            && text.text.contains("assistant")
                )
        )
    }));
    assert!(updates.iter().any(|notification| {
        matches!(
            &notification.update,
            v1::SessionUpdate::AgentMessageChunk(chunk)
                if matches!(
                    &chunk.content,
                    v1::ContentBlock::Text(text)
                        if text.text.contains("file:///not-a-resource")
                            && !text.text.contains("structured:resource:")
                )
        )
    }));
    assert!(updates.iter().any(|notification| {
        matches!(
            &notification.update,
            v1::SessionUpdate::ToolCallUpdate(update)
                if update.tool_call_id.to_string() == "call-failed"
                    && update.fields.status == Some(v1::ToolCallStatus::Failed)
        )
    }));
    assert!(updates.iter().any(|notification| {
        matches!(
            &notification.update,
            v1::SessionUpdate::ToolCall(call)
                if call.tool_call_id.to_string() == "call-1"
                    && call.status == v1::ToolCallStatus::Pending
        )
    }));
    assert!(updates.iter().any(|notification| {
        matches!(
            &notification.update,
            v1::SessionUpdate::ToolCallUpdate(update)
                if update.tool_call_id.to_string() == "call-1"
                    && update.fields.status == Some(v1::ToolCallStatus::Completed)
        )
    }));
    Ok(())
}

// ── Shared connection runtime (supreme plan 05, extract-shared-acp-runtime) ──

use echo_agent::acp::{
    AcpConnectionServices, AcpLedgerLimits, AcpSession, RunEventObserver, RunStartSpec,
    SessionRegistry,
};
use echo_agent::runtime::{TurnDeliveryOutcome, TurnMode, TurnOutcome, TurnRequest};
use echo_agent::state::journal::{
    EventJournal, JournalBatchAppendError, JournalBatchAppendResult, JournalBatchLookup,
    JournalIdentity, PreparedJournalBatch,
};

#[derive(Default)]
struct RecordingObserver {
    events: Arc<Mutex<Vec<(u64, String)>>>,
    fail_on_stream: Arc<Mutex<Option<String>>>,
    fail_on_terminal: Arc<Mutex<bool>>,
}

#[async_trait::async_trait]
impl RunEventObserver for RecordingObserver {
    async fn on_committed_event(&self, envelope: &echo_agent::agent::EventEnvelope) -> Result<()> {
        if let Some(fail_stream) = self
            .fail_on_stream
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            && envelope.stream_id.as_str() == fail_stream
        {
            return Err(ReactError::Other(
                "observer rejected the stream".to_string(),
            ));
        }
        if *self
            .fail_on_terminal
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            && TurnOutcome::classify(&envelope.payload).is_some()
        {
            return Err(ReactError::Other(
                "observer rejected the terminal".to_string(),
            ));
        }
        self.events
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((envelope.sequence, format!("{:?}", envelope.payload)));
        Ok(())
    }
}

/// Journal hook that always fails: delivery fails without rewriting a producer terminal.
#[derive(Default)]
struct FailingJournal;

impl EventJournal<echo_agent::agent::EventEnvelope> for FailingJournal {
    fn journal_identity(&self) -> &JournalIdentity {
        static IDENTITY: OnceLock<JournalIdentity> = OnceLock::new();
        IDENTITY.get_or_init(JournalIdentity::new)
    }

    fn append_batch(
        &self,
        batch: PreparedJournalBatch<echo_agent::agent::EventEnvelope>,
    ) -> JournalBatchAppendResult<echo_agent::agent::EventEnvelope> {
        Err(JournalBatchAppendError::NotCommitted {
            batch,
            error: "fixture journal is unavailable".to_string(),
        })
    }

    fn lookup_batch(
        &self,
        _batch: &PreparedJournalBatch<echo_agent::agent::EventEnvelope>,
    ) -> echo_agent::error::Result<JournalBatchLookup<echo_agent::agent::EventEnvelope>> {
        Ok(JournalBatchLookup::Absent)
    }

    fn next_sequence(&self) -> u64 {
        1
    }

    fn last_sequence(&self) -> u64 {
        0
    }

    fn replay_after(
        &self,
        _after_sequence: u64,
        _limit: usize,
    ) -> echo_agent::error::Result<
        Vec<echo_state::journal::JournalRecord<echo_agent::agent::EventEnvelope>>,
    > {
        Ok(Vec::new())
    }
}

async fn shared_services(
    factory: impl AcpSessionFactory,
) -> Result<(Arc<AcpConnectionServices>, String)> {
    let registry = Arc::new(SessionRegistry::new(Arc::new(factory), 8));
    registry
        .initialize(agent_client_protocol::schema::v1::ClientCapabilities::default())
        .await;
    let config = Arc::new(AcpAdapterConfig::default());
    let services = Arc::new(AcpConnectionServices::new(registry.clone(), config));
    let session_id = registry
        .create(agent_client_protocol::schema::v1::NewSessionRequest::new(
            absolute_test_path("shared-session")?,
        ))
        .await?;
    Ok((services, session_id.to_string()))
}

fn extension_spec(
    session: &Arc<AcpSession>,
    observer: Option<Arc<dyn RunEventObserver>>,
) -> Result<RunStartSpec> {
    let active = session.begin_turn()?;
    let run_id = active.turn.id().to_string();
    let stream_id = format!("stream-{run_id}");
    let identity = echo_agent::agent::EventIdentity::new(stream_id.clone(), run_id.clone())?;
    let turn = TurnRequest::new(identity, "extension run")
        .mode(TurnMode::Chat)
        .cancel(active.turn.cancellation());
    Ok(RunStartSpec {
        session: session.clone(),
        active,
        run_id,
        stream_id,
        turn,
        projector: None,
        journal: None,
        observers: observer.into_iter().collect(),
    })
}

#[tokio::test]
async fn shared_authority_settles_extension_runs_with_exactly_one_terminal() -> Result<()> {
    let test = TestFactory::new();
    let (services, session_id) = shared_services(test.session_factory()).await?;
    let session = services
        .sessions()
        .get(&agent_client_protocol::schema::v1::SessionId::new(
            session_id.clone(),
        ))
        .await
        .ok_or_else(|| ReactError::Other("session missing".to_string()))?;
    let observer = Arc::new(RecordingObserver::default());
    let spec = extension_spec(
        &session,
        Some(observer.clone() as Arc<dyn RunEventObserver>),
    )?;
    let run_id = spec.run_id.clone();
    let (entry, task) = services.prepare_run(spec).await?;
    tokio::spawn(task);
    let receipt = entry.wait_receipt().await;
    assert_eq!(receipt.outcome, TurnOutcome::Completed);
    assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
    assert!(services.run(&run_id).await.is_some());
    let events = observer
        .events
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    assert!(!events.is_empty());
    // Sequences are contiguous from one; the last event is the single terminal.
    for (index, (sequence, _)) in events.iter().enumerate() {
        assert_eq!(*sequence, (index + 1) as u64);
    }
    Ok(())
}

#[tokio::test]
async fn one_active_run_slot_is_shared_across_entry_points() -> Result<()> {
    let test = TestFactory::new();
    let (services, session_id) = shared_services(test.session_factory()).await?;
    let acp_session_id = agent_client_protocol::schema::v1::SessionId::new(session_id.clone());
    let session = services
        .sessions()
        .get(&acp_session_id)
        .await
        .ok_or_else(|| ReactError::Other("session missing".to_string()))?;
    let first = session.begin_turn()?;
    assert!(
        session.begin_turn().is_err(),
        "a second concurrent run slot must be refused regardless of entry point"
    );
    drop(first);
    // After the lease drops the slot is free again.
    let second = session.begin_turn()?;
    drop(second);
    Ok(())
}

#[tokio::test]
async fn journal_failure_before_terminal_fails_execution_and_delivery() -> Result<()> {
    let test = TestFactory::new();
    let (services, session_id) = shared_services(test.session_factory()).await?;
    let acp_session_id = agent_client_protocol::schema::v1::SessionId::new(session_id.clone());
    let session = services
        .sessions()
        .get(&acp_session_id)
        .await
        .ok_or_else(|| ReactError::Other("session missing".to_string()))?;
    let mut spec = extension_spec(&session, None)?;
    spec.journal = Some(Arc::new(FailingJournal));
    let (entry, task) = services.prepare_run(spec).await?;
    tokio::spawn(task);
    let receipt = entry.wait_receipt().await;
    assert!(matches!(receipt.outcome, TurnOutcome::Failed(_)));
    assert!(matches!(receipt.delivery, TurnDeliveryOutcome::Failed(_)));
    Ok(())
}

#[tokio::test]
async fn observer_failure_is_delivery_failure_without_rewriting_execution() -> Result<()> {
    let test = TestFactory::new();
    let (services, session_id) = shared_services(test.session_factory()).await?;
    let session = services
        .sessions()
        .get(&agent_client_protocol::schema::v1::SessionId::new(
            session_id,
        ))
        .await
        .ok_or_else(|| ReactError::Other("session missing".to_string()))?;
    let observer = Arc::new(RecordingObserver::default());
    let spec = extension_spec(
        &session,
        Some(observer.clone() as Arc<dyn RunEventObserver>),
    )?;

    *observer
        .fail_on_terminal
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = true;
    let (entry, task) = services.prepare_run(spec).await?;
    tokio::spawn(task);
    let receipt = entry.wait_receipt().await;
    assert_eq!(receipt.outcome, TurnOutcome::Completed);
    assert!(matches!(receipt.delivery, TurnDeliveryOutcome::Failed(_)));
    Ok(())
}

#[tokio::test]
async fn cancel_run_routes_through_the_shared_cancellation_authority() -> Result<()> {
    let test = TestFactory::new();
    let (services, session_id) = shared_services(test.session_factory()).await?;
    let acp_session_id = agent_client_protocol::schema::v1::SessionId::new(session_id.clone());
    let session = services
        .sessions()
        .get(&acp_session_id)
        .await
        .ok_or_else(|| ReactError::Other("session missing".to_string()))?;
    let spec = extension_spec(&session, None)?;
    let run_id = spec.run_id.clone();
    {
        // Park the run on the gate ("wait-for-release" parks until cancel).
        let mut spec = spec;
        if let TurnRequest {
            input: echo_agent::runtime::TurnInput::Text(text),
            ..
        } = &mut spec.turn
        {
            *text = "wait-for-release".to_string();
        }
        let (entry, task) = services.prepare_run(spec).await?;
        tokio::spawn(task);
        assert!(services.cancel_run(&run_id).await);
        let receipt = entry.wait_receipt().await;
        assert_eq!(receipt.outcome, TurnOutcome::Cancelled);
    }
    // Unknown run ids cancel nothing.
    assert!(!services.cancel_run("missing-run").await);
    Ok(())
}

#[tokio::test]
async fn ledger_bounds_retain_a_floor_and_clamp_memory() -> Result<()> {
    let test = TestFactory::new();
    let (services, session_id) = shared_services(test.session_factory()).await?;
    let acp_session_id = agent_client_protocol::schema::v1::SessionId::new(session_id.clone());
    let session = services
        .sessions()
        .get(&acp_session_id)
        .await
        .ok_or_else(|| ReactError::Other("session missing".to_string()))?;
    let _session = session;
    let ledger = echo_agent::acp::EventLedger::new(
        AcpLedgerLimits {
            max_events: 2,
            max_bytes: 1_000_000,
        },
        None,
    );
    let identity = echo_agent::agent::EventIdentity::new("ledger-stream", "ledger-turn")?;
    for sequence in 1..=4_u64 {
        let envelope = echo_agent::agent::EventEnvelope::new(
            &identity,
            sequence,
            None,
            fixture_block(
                "ledger-request",
                "ledger-block",
                sequence,
                AssistantPhase::Commentary,
                &format!("event-{sequence}"),
            ),
        )?;
        ledger.commit(envelope)?;
    }
    assert_eq!(ledger.last_sequence(), 4);
    assert_eq!(ledger.retained_floor(), 3, "oldest retained sequence");
    let after = ledger.events_after(0);
    assert_eq!(after.len(), 2);
    assert_eq!(after.first().map(|event| event.sequence), Some(3));
    Ok(())
}

#[tokio::test]
async fn text_only_agent_cannot_use_retired_plain_text_fallback()
-> agent_client_protocol::Result<()> {
    let updates = Arc::new(Mutex::new(Vec::<v1::SessionNotification>::new()));
    let captured = updates.clone();
    let adapter = AcpAgentAdapter::new(|_context: AcpSessionContext| async {
        Ok(Box::new(TextOnlyAgent) as Box<dyn Agent>)
    });
    let _close_owner = adapter.close_owner();
    Client
        .builder()
        .on_receive_notification(
            async move |notification: v1::SessionNotification,
                        _connection: ConnectionTo<AcpRole>| {
                captured
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(adapter, async move |connection| {
            connection
                .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(v1::NewSessionRequest::new(
                    absolute_test_path("acp-rejected-text-only")
                        .map_err(Error::into_internal_error)?,
                ))
                .block_task()
                .await?
                .session_id;
            for text in ["first original 中文", "second original 🌊"] {
                let response = connection
                    .send_request(v1::PromptRequest::new(
                        session.clone(),
                        vec![v1::ContentBlock::Text(v1::TextContent::new(text))],
                    ))
                    .block_task()
                    .await;
                assert_eq!(
                    response.err().map(|error| error.code),
                    Some(ErrorCode::InternalError)
                );
            }
            Ok(())
        })
        .await?;
    let observed: Vec<String> = updates
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter_map(|notification| match &notification.update {
            v1::SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                v1::ContentBlock::Text(text) => Some(text.text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert!(
        observed.is_empty(),
        "退役的text-only fallback不得产生成功正文"
    );
    Ok(())
}

struct TypedSetupFailureAgent;
impl Agent for TypedSetupFailureAgent {
    fn name(&self) -> &str {
        "typed-setup-failure"
    }
    fn model_name(&self) -> &str {
        "none"
    }
    fn system_prompt(&self) -> &str {
        ""
    }
    fn chat_stream_message_with_invocation_context<'a>(
        &'a self,
        _message: Message,
        _cancel: CancellationToken,
        _invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async {
            Err(ReactError::Other(
                "original typed setup failure".to_string(),
            ))
        })
    }
}

#[derive(Default)]
struct CanonicalFailureSink(Mutex<Vec<echo_agent::agent::EventEnvelope>>);
#[async_trait::async_trait]
impl echo_agent::runtime::EventSink for CanonicalFailureSink {
    async fn on_event(
        &self,
        envelope: echo_agent::agent::EventEnvelope,
    ) -> Result<echo_agent::runtime::SinkControl> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(envelope);
        Ok(echo_agent::runtime::SinkControl::Continue)
    }
}

#[tokio::test]
async fn typed_setup_error_keeps_original_failed_receipt_without_inventing_source_status()
-> Result<()> {
    let turn_id = "original-canonical-typed-turn";
    let identity = echo_agent::agent::EventIdentity::for_chat(
        None,
        turn_id.to_string(),
        "original-message".to_string(),
        None,
    )?;
    let request = echo_agent::runtime::TurnRequest::new(identity, "original text").invocation(
        echo_agent::agent::AgentInvocationContext {
            runtime: Some(echo_agent::tools::ExternalRunContext {
                turn_id: Some(turn_id.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let sink = CanonicalFailureSink::default();
    let receipt = echo_agent::runtime::AgentTurnDriver
        .drive(&TypedSetupFailureAgent, request, &sink)
        .await?;
    assert_eq!(receipt.turn_id.as_str(), turn_id);
    assert!(
        matches!(receipt.outcome, echo_agent::runtime::TurnOutcome::Failed(ref failure) if failure.message == "original typed setup failure")
    );
    assert!(receipt.final_answer.is_none());
    let events = sink.0.lock().unwrap_or_else(|e| e.into_inner());
    // 源未 admission，不得伪造 Started/Failed；setup failure仍由原driver记录投递事实。
    assert!(
        !events
            .iter()
            .any(|event| matches!(&event.payload, AgentEvent::ExecutionStatus(_)))
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.payload.is_execution_terminal())
            .count(),
        0
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(&event.payload,
        AgentEvent::Error { failure, .. }
            if failure.message == "original typed setup failure"))
            .count(),
        1
    );
    assert!(events.iter().all(|event| event.turn_id.as_str() == turn_id));
    assert!(events.iter().any(|event| matches!(&event.payload, AgentEvent::Error { failure, .. } if failure.message == "original typed setup failure")));
    Ok(())
}
