//! Minimal source-built ACP Agent composed from the public echo-agent facade.
//!
//! This teaching example uses a deterministic Agent so it needs no model
//! credentials. It is not the configurable `echo-agent-sdk-host` binary from
//! the next SDK delivery stage.

use agent_client_protocol::{ConnectTo as _, Stdio};
use echo_agent::acp::{AcpAgentAdapter, AcpSessionContext};
use echo_agent::agent::{Agent, AgentEvent, CancellationToken};
use echo_agent::error::Result;
use echo_agent::llm::types::Message;
use futures::future::BoxFuture;
use futures::stream::{self, BoxStream, StreamExt};
use std::sync::Mutex;

struct ExampleSessionAgent {
    session_id: String,
    messages: Mutex<Vec<String>>,
}

impl ExampleSessionAgent {
    fn chat_stream_owned<'a>(
        &'a self,
        message: String,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let answer = {
            let mut messages = self
                .messages
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            messages.push(message.clone());
            format!("{} turn {}: {}", self.session_id, messages.len(), message)
        };
        Box::pin(async move { Ok(stream::iter(vec![Ok(AgentEvent::FinalAnswer(answer))]).boxed()) })
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
        self.chat_stream_owned(message.to_string())
    }

    fn chat_stream_message_with_cancel<'a>(
        &'a self,
        message: Message,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.chat_stream_owned(message.content.as_text().unwrap_or_default())
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

    adapter.connect_to(Stdio::new()).await?;
    Ok(())
}
