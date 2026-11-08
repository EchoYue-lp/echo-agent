//! 实际ReactAgent→原snapshot→create_llm_stream回归；不调用helper绕gate。
//! RecordingClient只替代网络，模型/协议事实委托真实配置client，不注入capability布尔。
use crate::agent::{
    Agent, AgentEvent, AgentRunSnapshot, ExecutionStatusKind, ReactAgent, ReactAgentBuilder,
};
use crate::error::Result;
use crate::llm::{
    ChatChunk, ChatRequest, ChatResponse, LlmApiProtocol, LlmClient, LlmConfig, ResponseFormat,
};
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct RecordingClient {
    configured: Arc<dyn LlmClient>,
    calls: AtomicUsize,
}
impl LlmClient for RecordingClient {
    fn chat(&self, _request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ChatResponse {
                message: crate::llm::types::Message::assistant(r#"{"ok":true}"#.into()),
                finish_reason: Some("stop".into()),
                usage: None,
                raw: Default::default(),
            })
        })
    }
    fn chat_stream(
        &self,
        _request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(futures::stream::iter(vec![Ok(ChatChunk {
                delta: crate::llm::types::DeltaMessage {
                    role: Some("assistant".into()),
                    content: Some(r#"{"ok":true}"#.into()),
                    ..Default::default()
                },
                finish_reason: Some("stop".into()),
                usage: None,
            })])
            .boxed())
        })
    }
    fn model_name(&self) -> &str {
        self.configured.model_name()
    }
    fn provider_name(&self) -> Option<&str> {
        self.configured.provider_name()
    }
    fn capabilities(&self) -> crate::llm::ProviderCapabilities {
        self.configured.capabilities()
    }
    fn protocol_capabilities(&self) -> crate::llm::ProviderCapabilityOverride {
        self.configured.protocol_capabilities()
    }
    fn model_profile_resolution(&self) -> crate::llm::ModelProfileResolution {
        self.configured.model_profile_resolution()
    }
}

fn make_agent(
    protocol: LlmApiProtocol,
    format: ResponseFormat,
    streaming: bool,
) -> Result<(ReactAgent, Arc<RecordingClient>)> {
    let config = LlmConfig::for_provider(
        "deepseek",
        format!("https://api.deepseek.com/v1/{}", protocol.endpoint_path()),
        "fixture-only-not-a-live-token",
        "deepseek-flash",
        protocol,
    )?;
    // build_client只构造原协议client；记录器从不向这个URL发送请求，不读取凭据。
    let client = Arc::new(RecordingClient {
        configured: Arc::from(config.build_client()?),
        calls: AtomicUsize::new(0),
    });
    let agent = ReactAgentBuilder::new()
        .configured_llm_client(client.clone(), config)
        .auto_project_rules(false)
        .model_streaming(streaming)
        .response_format(format)
        .build()?;
    Ok((agent, client))
}

async fn execute(agent: &ReactAgent) -> Result<Vec<Result<AgentEvent>>> {
    Ok(agent
        .execute_stream("Return JSON with ok=true")
        .await?
        .collect::<Vec<_>>()
        .await)
}

fn approved_once(events: &[Result<AgentEvent>]) {
    assert!(events.iter().all(Result::is_ok));
    assert_eq!(events.iter().filter(|event|matches!(event,Ok(AgentEvent::FinalAnswer(answer)) if answer==r#"{"ok":true}"#)).count(),1);
    assert_eq!(events.iter().filter(|event|matches!(event,Ok(AgentEvent::ExecutionStatus(status)) if status.status==ExecutionStatusKind::Settled)).count(),1);
}

#[tokio::test]
async fn json_object_runtime_gate_deepseek_json_object_reaches_original_client_once() -> Result<()>
{
    for streaming in [false, true] {
        let (agent, client) = make_agent(
            LlmApiProtocol::ChatCompletions,
            ResponseFormat::JsonObject,
            streaming,
        )?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        assert!(
            !snapshot.config.supports_structured_output,
            "需先合仅Chat schema=false事实；不能用schema=true掩盖gate"
        );
        let events = execute(&agent).await?;
        assert_eq!(
            client.calls.load(Ordering::SeqCst),
            1,
            "实际Agent未穿过snapshot/create_llm_stream到同一client"
        );
        approved_once(&events);
        agent.close().await?;
    }
    Ok(())
}

#[tokio::test]
async fn json_object_runtime_gate_same_deepseek_json_schema_still_rejects_before_model()
-> Result<()> {
    for streaming in [false, true] {
        let format = ResponseFormat::json_schema(
            "ok",
            serde_json::json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}),
        );
        let (agent, client) = make_agent(LlmApiProtocol::ChatCompletions, format, streaming)?;
        assert!(
            !AgentRunSnapshot::from_agent(&agent)
                .config
                .supports_structured_output
        );
        let events = execute(&agent).await?;
        assert_eq!(
            client.calls.load(Ordering::SeqCst),
            0,
            "不支持的JsonSchema不应调用模型"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(AgentEvent::FinalAnswer(_))))
        );
        assert!(
            events
                .iter()
                .any(|event| event.is_err() || matches!(event, Ok(AgentEvent::Error { .. })))
        );
        agent.close().await?;
    }
    Ok(())
}

#[tokio::test]
async fn json_object_runtime_gate_responses_schema_policy_remains_reachable() -> Result<()> {
    for streaming in [false, true] {
        let format = ResponseFormat::json_schema(
            "ok",
            serde_json::json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}),
        );
        let (agent, client) = make_agent(LlmApiProtocol::Responses, format, streaming)?;
        assert!(
            AgentRunSnapshot::from_agent(&agent)
                .config
                .supports_structured_output
        );
        let events = execute(&agent).await?;
        assert_eq!(client.calls.load(Ordering::SeqCst), 1);
        approved_once(&events);
        agent.close().await?;
    }
    Ok(())
}
