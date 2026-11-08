//! Minimal source-built ACP Agent composed from the public echo-agent facade.
//!
//! This teaching example uses a deterministic Agent so it needs no model
//! credentials. It is not the configurable `echo-agent-sdk-host` binary from
//! the next SDK delivery stage.

use agent_client_protocol::Stdio;
use echo_agent::acp::{AcpAgentAdapter, AcpSessionContext};
use echo_agent::agent::{
    Agent, AgentEvent, AgentInvocationContext, AgentPhase, CancellationToken, EventIdentity,
    ExecutionStatus, ExecutionStatusKind,
};
use echo_agent::error::Result;
use echo_agent::llm::types::Message;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::sync::Mutex;

struct ExampleSessionAgent {
    session_id: String,
    messages: Mutex<Vec<Message>>,
}

impl ExampleSessionAgent {
    /// 确定性示例本身就是原生 source；状态来自本次执行，保留完整值到原 EOF。
    fn response_source<'a>(
        &'a self,
        answer: String,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            let identity = EventIdentity::from_invocation(&invocation)?;
            let execution_id = identity
                .execution_id
                .as_ref()
                .map(|value| value.as_str())
                .unwrap_or(identity.turn_id.as_str())
                .to_string();
            let request_id = identity
                .message_id
                .as_ref()
                .map(|value| value.as_str().to_string())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let status = move |kind, sequence| {
                AgentEvent::ExecutionStatus(ExecutionStatus {
                    adapter_generated: false,
                    execution_id: execution_id.clone(),
                    request_id: Some(request_id.clone()),
                    sequence,
                    phase: AgentPhase::Thinking,
                    status: kind,
                    reason: None,
                })
            };
            let source = async_stream::stream! {
                let _message = message;
                let _invocation = invocation;
                yield Ok(status(ExecutionStatusKind::Started, 0));
                if cancel.is_cancelled() {
                    yield Ok(status(ExecutionStatusKind::Cancelled, 1));
                    yield Ok(AgentEvent::Cancelled);
                    return;
                }
                yield Ok(AgentEvent::FinalAnswer(answer));
                yield Ok(status(ExecutionStatusKind::Settled, 1));
            };
            Ok(Box::pin(source) as BoxStream<'a, Result<AgentEvent>>)
        })
    }
}

impl Agent for ExampleSessionAgent {
    fn name(&self) -> &str {
        "example-acp-agent"
    }

    fn model_name(&self) -> &str {
        "deterministic-example"
    }

    fn system_prompt(&self) -> &str {
        "Echo ACP prompts for the learning example."
    }

    fn execute_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        // fresh execute 保留原来的无会话 echo 行为，不修改 chat 的 turn 计数。
        let answer = message.content.as_text().unwrap_or_default();
        self.response_source(answer, message, cancel, invocation)
    }

    fn chat_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let answer = {
            let mut messages = self
                .messages
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            messages.push(message.clone());
            format!(
                "{} turn {}: {}",
                self.session_id,
                messages.len(),
                message.content.as_text().unwrap_or_default()
            )
        };
        self.response_source(answer, message, cancel, invocation)
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let adapter = AcpAgentAdapter::new(|context: AcpSessionContext| async move {
        Ok(Box::new(ExampleSessionAgent {
            session_id: context.session_id.to_string(),
            messages: Mutex::new(Vec::new()),
        }) as Box<dyn Agent>)
    });

    let (close_owner, connection) = adapter.connect_retaining_close_owner(Stdio::new());
    let result = connection.await;
    if result.is_err() {
        close_owner.close().await?;
    }
    result?;
    Ok(())
}
