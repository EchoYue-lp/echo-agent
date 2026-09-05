#![cfg(feature = "acp")]

use agent_client_protocol::schema::{ProtocolVersion, v1};
use agent_client_protocol::{
    Agent as AcpRole, Channel, Client, ConnectTo as _, ConnectionTo, Error, ErrorCode,
};
use echo_agent::acp::{AcpAdapterConfig, AcpAgentAdapter, AcpSessionContext, AcpSessionFactory};
use echo_agent::agent::{Agent, AgentEvent, CancellationToken, ToolInvocation};
use echo_agent::error::AgentFailure;
use echo_agent::error::{ReactError, Result};
use echo_agent::llm::types::{ContentPart, Message, MessageContent};
use echo_agent::tools::{ToolFailure, ToolFailureCategory, ToolResult, ToolStreamEvent};
use futures::future::BoxFuture;
use futures::stream::{self, BoxStream, StreamExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

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
    started_notify: Notify,
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
}

impl Drop for RecordingAgent {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
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
            self.closed.store(true, Ordering::Release);
            Ok(())
        })
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

    fn chat_stream_with_cancel<'a>(
        &'a self,
        message: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let message = message.to_string();
        let id = self.id;
        let turn = self.turns.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        Box::pin(async move {
            if message == "wait-for-cancel" {
                let events = async_stream::stream! {
                    cancel.cancelled().await;
                    yield Ok(AgentEvent::Cancelled);
                };
                return Ok(Box::pin(events) as BoxStream<'a, Result<AgentEvent>>);
            }
            if message == "wait-for-release" {
                self.gate.started.fetch_add(1, Ordering::AcqRel);
                self.gate.started_notify.notify_waiters();
                let event = tokio::select! {
                    () = self.gate.release.notified() => {
                        AgentEvent::FinalAnswer(format!("agent-{id}-released"))
                    }
                    () = cancel.cancelled() => {
                        self.gate.cancelled.fetch_add(1, Ordering::AcqRel);
                        AgentEvent::Cancelled
                    }
                };
                return Ok(stream::iter(vec![Ok(event)]).boxed());
            }
            if message == "fail" {
                let failure = AgentFailure::message("acp-test", "scripted framework failure");
                return Ok(stream::iter(vec![Ok(AgentEvent::Error {
                    source: "acp-test".to_string(),
                    message: failure.message.clone(),
                    failure,
                })])
                .boxed());
            }
            if message == "tool" {
                return Ok(stream::iter(vec![
                    Ok(AgentEvent::ToolCall {
                        call_id: "call-1".to_string(),
                        invocation: ToolInvocation {
                            requested_name: "inspect".to_string(),
                            requested_args: serde_json::json!({"path": "README.md"}),
                            name: "inspect".to_string(),
                            args: serde_json::json!({"path": "README.md"}),
                            rewrites: Vec::new(),
                        },
                    }),
                    Ok(AgentEvent::ToolStream {
                        call_id: "call-1".to_string(),
                        name: "inspect".to_string(),
                        event: ToolStreamEvent::Progress {
                            message: "reading".to_string(),
                            percent: Some(50),
                        },
                    }),
                    Ok(AgentEvent::ToolResult {
                        call_id: "call-1".to_string(),
                        name: "inspect".to_string(),
                        result: ToolResult::success("done"),
                    }),
                    Ok(AgentEvent::FinalAnswer("tool-done".to_string())),
                ])
                .boxed());
            }
            if message == "tool-failure" {
                let mut failed = ToolResult::success("structured failure without error text");
                failed.success = false;
                failed.error = None;
                failed.failure = Some(ToolFailure::new(ToolFailureCategory::Permanent));
                return Ok(stream::iter(vec![
                    Ok(AgentEvent::ToolCall {
                        call_id: "call-failed".to_string(),
                        invocation: ToolInvocation {
                            requested_name: "inspect".to_string(),
                            requested_args: serde_json::json!({}),
                            name: "inspect".to_string(),
                            args: serde_json::json!({}),
                            rewrites: Vec::new(),
                        },
                    }),
                    Ok(AgentEvent::ToolResult {
                        call_id: "call-failed".to_string(),
                        name: "inspect".to_string(),
                        result: failed,
                    }),
                    Ok(AgentEvent::FinalAnswer("tool-failed".to_string())),
                ])
                .boxed());
            }
            if message == "many-small-updates" {
                return Ok(stream::iter(vec![
                    Ok(AgentEvent::Token("a".to_string())),
                    Ok(AgentEvent::Token("b".to_string())),
                    Ok(AgentEvent::Token("c".to_string())),
                    Ok(AgentEvent::FinalAnswer("abc".to_string())),
                ])
                .boxed());
            }
            let answer = format!("agent-{id}-turn-{turn}:{message}");
            Ok(stream::iter(vec![
                Ok(AgentEvent::ThinkStart),
                Ok(AgentEvent::Token("reasoning".to_string())),
                Ok(AgentEvent::ThinkEnd {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                }),
                Ok(AgentEvent::Token(answer.clone())),
                Ok(AgentEvent::FinalAnswer(answer)),
            ])
            .boxed())
        })
    }

    fn chat_stream_message_with_cancel<'a>(
        &'a self,
        message: Message,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let id = self.id;
        let turn = self.turns.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        let rendered = match message.content {
            MessageContent::Parts(parts) => parts
                .into_iter()
                .map(|part| match part {
                    ContentPart::Text { text } => format!("text:{text}"),
                    ContentPart::ResourceLink { resource } => format!(
                        "resource:{}",
                        serde_json::to_string(&resource).unwrap_or_default()
                    ),
                    ContentPart::ImageUrl { .. } => "image".to_string(),
                    ContentPart::File { name, .. } => format!("file:{name}"),
                })
                .collect::<Vec<_>>()
                .join("|"),
            content => content.as_text().unwrap_or_default(),
        };
        let answer = format!("agent-{id}-turn-{turn}-structured:{rendered}");
        Box::pin(async move { Ok(stream::iter(vec![Ok(AgentEvent::FinalAnswer(answer))]).boxed()) })
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

#[derive(Clone)]
struct TestFactory {
    next_id: Arc<AtomicUsize>,
    contexts: Arc<Mutex<Vec<AcpSessionContext>>>,
    closed: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
    gate: Arc<TurnGate>,
}

impl TestFactory {
    fn new() -> Self {
        Self {
            next_id: Arc::new(AtomicUsize::new(0)),
            contexts: Arc::new(Mutex::new(Vec::new())),
            closed: Arc::new(Mutex::new(Vec::new())),
            gate: Arc::new(TurnGate::default()),
        }
    }

    fn session_factory(&self) -> impl AcpSessionFactory {
        let factory = self.clone();
        move |context: AcpSessionContext| {
            let factory = factory.clone();
            async move {
                let id = factory.next_id.fetch_add(1, Ordering::AcqRel);
                let closed = Arc::new(AtomicBool::new(false));
                factory
                    .contexts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(context);
                factory
                    .closed
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(closed.clone());
                Ok(
                    Box::new(RecordingAgent::new(id, closed, factory.gate.clone()))
                        as Box<dyn Agent>,
                )
            }
        }
    }

    fn adapter(&self) -> AcpAgentAdapter {
        AcpAgentAdapter::new(self.session_factory())
    }

    fn adapter_with_config(&self, config: AcpAdapterConfig) -> Result<AcpAgentAdapter> {
        AcpAgentAdapter::with_config(self.session_factory(), config)
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

#[test]
fn acp_fixtures_use_official_typed_messages() -> agent_client_protocol::Result<()> {
    let prompt: v1::PromptRequest = serde_json::from_str(include_str!(
        "../contracts/sdk/fixtures/acp/v1/prompt-resource-link-valid.json"
    ))
    .map_err(Error::into_internal_error)?;
    assert_eq!(prompt.session_id, v1::SessionId::new("sess_fixture"));
    assert_eq!(prompt.prompt.len(), 2);

    let invalid_session: v1::NewSessionRequest = serde_json::from_str(include_str!(
        "../contracts/sdk/fixtures/acp/v1/session-relative-cwd-invalid.json"
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
                "../contracts/sdk/fixtures/acp/v1/session-relative-cwd-invalid.json"
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
                "../contracts/sdk/fixtures/acp/v1/prompt-resource-link-valid.json"
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
