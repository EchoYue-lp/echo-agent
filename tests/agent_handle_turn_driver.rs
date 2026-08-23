#![cfg(feature = "testing")]

use async_trait::async_trait;
use echo_agent::agent::{
    AgentHandle, AgentInvocationContext, CancellationToken, EventEnvelope, EventIdentity,
    ReactAgentBuilder,
};
use echo_agent::error::{ReactError, Result};
use echo_agent::llm::types::{ContentPart, ImageUrl, Message};
use echo_agent::runtime::{
    AgentTurnDriver, EventSink, SinkControl, TurnMode, TurnOutcome, TurnRequest,
};
use echo_agent::testing::MockLlmClient;
use echo_agent::tools::{Tool, ToolContext, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct DiscardSink;

#[derive(Clone)]
struct ContextProbe {
    observed: Arc<Mutex<Option<ObservedContext>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedContext {
    working_dir: Option<PathBuf>,
    cancellation_provided: bool,
}

impl Tool for ContextProbe {
    fn name(&self) -> &str {
        "context_probe"
    }

    fn description(&self) -> &str {
        "Record invocation context for a framework integration test"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        _parameters: ToolParameters,
        context: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            *self
                .observed
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(ObservedContext {
                working_dir: context.working_dir.clone(),
                cancellation_provided: context.cancel.is_some(),
            });
            Ok(ToolResult::success("context observed"))
        })
    }
}

#[async_trait]
impl EventSink for DiscardSink {
    async fn on_event(&self, _envelope: EventEnvelope) -> Result<SinkControl> {
        Ok(SinkControl::Continue)
    }
}

fn identity(label: &str) -> Result<EventIdentity> {
    EventIdentity::new(format!("stream-{label}"), format!("turn-{label}"))
}

fn multimodal_message(text: &str, image: &str) -> Message {
    Message::user_multimodal(vec![
        ContentPart::Text {
            text: text.to_string(),
        },
        ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: image.to_string(),
                detail: Some("low".to_string()),
            },
        },
    ])
}

fn contains_text(messages: &[Message], expected: &str) -> bool {
    messages.iter().any(|message| {
        message
            .content
            .as_text()
            .is_some_and(|text| text.contains(expected))
    })
}

#[tokio::test]
async fn borrowed_handle_driver_preserves_chat_invocation_and_execute_semantics() -> Result<()> {
    let llm = Arc::new(
        MockLlmClient::new()
            .then_tool_call("probe-1", "context_probe", "{}")
            .with_response("chat-answer")
            .with_response("execute-answer"),
    );
    let observed_context = Arc::new(Mutex::new(None));
    let agent = ReactAgentBuilder::new()
        .llm_client(llm.clone())
        .tool(Box::new(ContextProbe {
            observed: observed_context.clone(),
        }))
        .system_prompt("You are a test assistant.")
        .build()?;
    let handle = AgentHandle::new(agent);
    let working_dir = PathBuf::from("/tmp/echo-agent-handle-driver");
    let invocation_cancel = CancellationToken::new();
    let chat_request = TurnRequest::from_message(
        identity("borrowed-chat")?,
        multimodal_message("inspect this image", "data:image/png;base64,aW1hZ2U="),
    )
    .invocation(AgentInvocationContext {
        working_dir: Some(working_dir.clone()),
        cancel: Some(invocation_cancel.clone()),
        history: Some(vec![
            Message::user("prior question".to_string()),
            Message::assistant("prior answer".to_string()),
        ]),
        ..AgentInvocationContext::default()
    });

    let chat_receipt = handle
        .read_async(|agent| {
            Box::pin(async move {
                AgentTurnDriver
                    .drive(agent, chat_request, &DiscardSink)
                    .await
            })
        })
        .await;
    assert_eq!(chat_receipt.outcome, TurnOutcome::Completed);
    assert_eq!(chat_receipt.final_answer.as_deref(), Some("chat-answer"));
    assert!(!invocation_cancel.is_cancelled());
    assert_eq!(
        *observed_context
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
        Some(ObservedContext {
            working_dir: Some(working_dir),
            cancellation_provided: true,
        })
    );

    let calls = llm.all_calls();
    let chat_messages = calls
        .first()
        .ok_or_else(|| ReactError::Other("missing borrowed chat LLM call".to_string()))?;
    assert!(contains_text(chat_messages, "prior question"));
    assert!(contains_text(chat_messages, "prior answer"));
    assert!(contains_text(chat_messages, "inspect this image"));
    assert!(chat_messages.iter().any(|message| {
        message.content.parts().is_some_and(|parts| {
            parts.iter().any(|part| {
                matches!(
                    part,
                    ContentPart::ImageUrl { image_url }
                        if image_url.url == "data:image/png;base64,aW1hZ2U="
                )
            })
        })
    }));

    let execute_request = TurnRequest::from_message(
        identity("borrowed-execute")?,
        multimodal_message("fresh execute task", "data:image/png;base64,dGFzaw=="),
    )
    .mode(TurnMode::Execute);
    let execute_receipt = handle
        .read_async(|agent| {
            Box::pin(async move {
                AgentTurnDriver
                    .drive(agent, execute_request, &DiscardSink)
                    .await
            })
        })
        .await;
    assert_eq!(execute_receipt.outcome, TurnOutcome::Completed);
    assert_eq!(
        execute_receipt.final_answer.as_deref(),
        Some("execute-answer")
    );

    let calls = llm.all_calls();
    let execute_messages = calls
        .get(2)
        .ok_or_else(|| ReactError::Other("missing borrowed execute LLM call".to_string()))?;
    assert!(contains_text(execute_messages, "fresh execute task"));
    assert!(!contains_text(execute_messages, "inspect this image"));
    assert!(!contains_text(execute_messages, "prior question"));
    Ok(())
}

#[tokio::test]
async fn shared_agent_compatibility_forwards_structured_chat_and_execute() -> Result<()> {
    let llm = Arc::new(
        MockLlmClient::new()
            .with_response("shared-chat")
            .with_response("shared-execute"),
    );
    let handle = AgentHandle::new(
        ReactAgentBuilder::new()
            .llm_client(llm)
            .system_prompt("You are a compatibility test assistant.")
            .build()?,
    );
    let shared = handle.as_shared_agent().await;

    let chat_request = TurnRequest::from_message(
        identity("shared-chat")?,
        multimodal_message("shared chat input", "data:image/png;base64,Y2hhdA=="),
    )
    .invocation(AgentInvocationContext {
        working_dir: Some(PathBuf::from("/tmp/shared-agent-chat")),
        history: Some(vec![Message::user("shared history".to_string())]),
        cancel: Some(CancellationToken::new()),
        ..AgentInvocationContext::default()
    });
    let chat_receipt = AgentTurnDriver
        .drive(shared.as_ref(), chat_request, &DiscardSink)
        .await;
    assert_eq!(chat_receipt.outcome, TurnOutcome::Completed);
    assert_eq!(chat_receipt.final_answer.as_deref(), Some("shared-chat"));

    let execute_request = TurnRequest::from_message(
        identity("shared-execute")?,
        multimodal_message("shared execute input", "data:image/png;base64,ZXhlYw=="),
    )
    .mode(TurnMode::Execute)
    .invocation(AgentInvocationContext {
        cancel: Some(CancellationToken::new()),
        ..AgentInvocationContext::default()
    });
    let execute_receipt = AgentTurnDriver
        .drive(shared.as_ref(), execute_request, &DiscardSink)
        .await;
    assert_eq!(execute_receipt.outcome, TurnOutcome::Completed);
    assert_eq!(
        execute_receipt.final_answer.as_deref(),
        Some("shared-execute")
    );
    Ok(())
}
