use super::ReactAgent;
#[cfg(feature = "subagent")]
use crate::agent::ReactAgentBuilder;
use crate::agent::config::{AgentConfig, DEFAULT_TOKEN_LIMIT};
#[cfg(feature = "subagent")]
use crate::agent::subagent::SubagentBuilder;
#[cfg(feature = "subagent")]
use crate::agent::subagent::SubagentRegistry;
use crate::agent::{Agent, AgentHandle};
use crate::llm::types::{Message, Role};
#[cfg(feature = "shell")]
use crate::sandbox::SandboxManager;
#[cfg(feature = "shell")]
use crate::skills::builtin::ShellSkill;
use crate::skills::external::loader::DiscoveryScope;
use crate::skills::hooks::{HookAction, HookEvent, HookRule, HooksDefinition};
use crate::testing::{FailingMockAgent, MockAgent, MockTool};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ── ReactAgent::reset() ───────────────────────────────────────────────────────

/// reset() should clear all messages, keeping only the system prompt (1 message)
#[tokio::test]
async fn react_agent_reset_clears_to_system_only() {
    let config = AgentConfig::new("test-model", "test_agent", "You are a test assistant");
    let agent = ReactAgent::new(config);

    let (count, _) = agent.context_stats().await;
    assert_eq!(count, 1, "Initially should only have 1 system message");

    agent
        .memory
        .context
        .lock()
        .await
        .push(Message::user("Hello".to_string()));
    agent
        .memory
        .context
        .lock()
        .await
        .push(Message::assistant("Hello!".to_string()));
    agent
        .memory
        .context
        .lock()
        .await
        .push(Message::user("Goodbye".to_string()));
    let (count_after_push, _) = agent.context_stats().await;
    assert_eq!(
        count_after_push, 4,
        "After appending should have 4 messages"
    );

    agent.reset().await;
    let (count_after_reset, _) = agent.context_stats().await;
    assert_eq!(
        count_after_reset, 1,
        "After reset() should only have 1 system message"
    );
}

/// Multiple consecutive reset() calls should be idempotent, not producing duplicate system prompts
#[tokio::test]
async fn react_agent_reset_is_idempotent() {
    let config = AgentConfig::new("test-model", "test_agent", "System prompt");
    let agent = ReactAgent::new(config);

    agent.reset().await;
    agent.reset().await;
    agent.reset().await;

    let (count, _) = agent.context_stats().await;
    assert_eq!(
        count, 1,
        "After multiple reset() calls should still only have 1 system message"
    );
}

/// After reset() the system prompt content should remain unchanged
#[tokio::test]
async fn react_agent_reset_preserves_system_prompt() {
    let system = "This is a custom system prompt";
    let config = AgentConfig::new("test-model", "agent", system);
    let agent = ReactAgent::new(config);

    agent
        .memory
        .context
        .lock()
        .await
        .push(Message::user("Some random message".to_string()));
    agent.reset().await;

    let messages = agent.memory.context.lock().await.messages().to_vec();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, Role::System);
    assert_eq!(messages[0].content.as_text_ref().unwrap_or(""), system);
}

// ── ReactAgent tool registration tests ─────────────────────────────────────────

/// Note: ReactAgent::new automatically registers FinalAnswerTool
#[test]
fn react_agent_add_tool_enables_tool_flag() {
    let config = AgentConfig::minimal("test-model", "helper");
    assert!(
        !config.is_tool_enabled(),
        "minimal config does not enable tools by default"
    );

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(MockTool::new("test_tool")));

    assert!(
        agent.config().is_tool_enabled(),
        "after add_tool, tools should be enabled"
    );
    // FinalAnswerTool + test_tool
    let tool_names = agent.tool_names();
    assert!(tool_names.contains(&String::from("test_tool")));
    assert!(tool_names.contains(&String::from("final_answer")));
}

#[test]
fn react_agent_add_tools_batch() {
    let config = AgentConfig::minimal("test-model", "helper");
    let mut agent = ReactAgent::new(config);

    let tools: Vec<Box<dyn crate::tools::Tool>> = vec![
        Box::new(MockTool::new("tool1")),
        Box::new(MockTool::new("tool2")),
        Box::new(MockTool::new("tool3")),
    ];

    agent.add_tools(tools);

    let tool_names = agent.tool_names();
    // built-in tools + 3 custom tools
    assert!(tool_names.len() >= 4);
    assert!(tool_names.contains(&String::from("tool1")));
    assert!(tool_names.contains(&String::from("tool2")));
    assert!(tool_names.contains(&String::from("tool3")));
}

#[test]
fn react_agent_add_tools_empty_vec() {
    let config = AgentConfig::minimal("test-model", "helper");
    let mut agent = ReactAgent::new(config);

    agent.add_tools(vec![]);

    assert!(
        !agent.config().is_tool_enabled(),
        "empty tool list should not modify enable_tool"
    );
}

#[test]
fn react_agent_add_tools_with_allowed_list() {
    let config = AgentConfig::minimal("test-model", "helper")
        .allowed_tools(vec!["allowed_tool".to_string()]);
    let mut agent = ReactAgent::new(config);

    let tools: Vec<Box<dyn crate::tools::Tool>> = vec![
        Box::new(MockTool::new("allowed_tool")),
        Box::new(MockTool::new("blocked_tool")),
    ];

    agent.add_tools(tools);

    let tool_names = agent.tool_names();
    // built-in tools + allowed_tool (whitelist only filters user-added tools)
    assert!(tool_names.len() >= 2);
    assert!(tool_names.contains(&String::from("allowed_tool")));
}

// ── ReactAgent getter method tests ─────────────────────────────────────────────

#[test]
fn react_agent_tool_names() {
    let config = AgentConfig::minimal("test-model", "helper");
    let mut agent = ReactAgent::new(config);

    // ReactAgent::new registers built-in tools (at least FinalAnswerTool)
    let initial_len = agent.tool_names().len();
    assert!(initial_len >= 1);

    agent.add_tool(Box::new(MockTool::new("tool1")));
    agent.add_tool(Box::new(MockTool::new("tool2")));

    let names = agent.tool_names();
    assert_eq!(names.len(), initial_len + 2);
}

#[test]
fn react_agent_skill_names() {
    let config = AgentConfig::minimal("test-model", "helper");
    let agent = ReactAgent::new(config);

    assert!(
        agent.skill_names().is_empty(),
        "Initially should have no skills"
    );
}

#[test]
fn react_agent_mcp_server_names() {
    let config = AgentConfig::minimal("test-model", "helper");
    let agent = ReactAgent::new(config);

    assert!(
        agent.mcp_server_names().is_empty(),
        "Initially should have no MCP servers"
    );
}

#[tokio::test]
async fn react_agent_get_messages() {
    let config = AgentConfig::new("test-model", "agent", "You are helpful");
    let agent = ReactAgent::new(config);

    let messages = agent.get_messages().await;
    assert_eq!(messages.len(), 1, "Initially only has system message");
    assert_eq!(messages[0].role, Role::System);

    agent
        .memory
        .context
        .lock()
        .await
        .push(Message::user("Hello".to_string()));
    let messages = agent.get_messages().await;
    assert_eq!(messages.len(), 2);
}

#[tokio::test]
async fn react_agent_context_stats() {
    let config = AgentConfig::new("test-model", "agent", "System prompt");
    let agent = ReactAgent::new(config);

    let (count, _tokens) = agent.context_stats().await;
    assert_eq!(count, 1);

    agent
        .memory
        .context
        .lock()
        .await
        .push(Message::user("This is a test message".to_string()));
    let (count, tokens) = agent.context_stats().await;
    assert_eq!(count, 2);
    assert!(tokens > 0, "token estimate should be greater than 0");
}

// ── ReactAgent configuration tests ─────────────────────────────────────────────

#[test]
fn react_agent_set_model() {
    let config = AgentConfig::minimal("model1", "helper");
    let mut agent = ReactAgent::new(config);

    assert_eq!(agent.model_name(), "model1");

    agent.set_model("model2");

    assert_eq!(agent.model_name(), "model2");
}

#[tokio::test]
async fn react_agent_set_system_prompt() {
    let config = AgentConfig::minimal("test-model", "helper");
    let agent = ReactAgent::new(config);

    let original_prompt = agent.system_prompt().to_string();
    assert_eq!(original_prompt, "helper");

    // set_system_prompt stores a runtime override in mutable_system_prompt.
    // The system_prompt() getter returns the base config prompt (cannot return
    // a reference into RwLock). The override is applied at turn-start via
    // build_system_prompt().
    agent.set_system_prompt("New system prompt");

    // Base prompt is unchanged (by design — getter returns config value)
    assert_eq!(agent.system_prompt(), "helper");

    // The override IS stored internally
    let override_val = agent.mutable_system_prompt.read().unwrap();
    assert_eq!(override_val.as_deref(), Some("New system prompt"));
}

#[test]
fn react_agent_name() {
    let config = AgentConfig::new("model", "my_agent", "prompt");
    let agent = ReactAgent::new(config);

    assert_eq!(agent.name(), "my_agent");
}

#[test]
fn react_agent_model_name() {
    let config = AgentConfig::new("qwen3-max", "agent", "prompt");
    let agent = ReactAgent::new(config);

    assert_eq!(agent.model_name(), "qwen3-max");
}

#[test]
fn react_agent_system_prompt() {
    let config = AgentConfig::new("model", "agent", "Be helpful");
    let agent = ReactAgent::new(config);

    assert_eq!(agent.system_prompt(), "Be helpful");
}

// ── ReactAgent callback tests ──────────────────────────────────────────────────

/// Simple callback counter
struct CounterCallback {
    count: AtomicUsize,
}

impl CounterCallback {
    fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
        }
    }

    #[allow(dead_code)]
    fn get_count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

impl crate::agent::AgentCallback for CounterCallback {
    fn on_think_start<'a>(
        &'a self,
        _agent: &'a str,
        _messages: &'a [Message],
    ) -> futures::future::BoxFuture<'a, ()> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }

    fn on_final_answer<'a>(
        &'a self,
        _agent: &'a str,
        _answer: &'a str,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}

#[test]
fn react_agent_add_callback() {
    let config = AgentConfig::minimal("test-model", "helper");
    let mut agent = ReactAgent::new(config);

    let callback = Arc::new(CounterCallback::new());
    agent.add_callback(callback.clone());

    // Verify the callback has been added (by checking internal state)
    // Since callbacks are private, we can only verify through execution
    // Here we simply verify the method does not panic
}

// ── Agent trait contract ───────────────────────────────────────────────────────

/// reset() is callable via &mut dyn Agent (trait object safety verification)
#[tokio::test]
async fn trait_reset_callable_via_dyn_agent() {
    let agent: Box<dyn Agent> = Box::new(
        MockAgent::new("mock")
            .with_response("r1")
            .with_response("r2"),
    );

    let r1 = agent.chat("msg1").await.unwrap();
    assert_eq!(r1, "r1");

    agent.reset().await;

    let r2 = agent.chat("msg2").await.unwrap();
    assert_eq!(r2, "r2");
}

#[test]
fn trait_name_callable() {
    let agent: Box<dyn Agent> = Box::new(MockAgent::new("test_agent"));

    assert_eq!(agent.name(), "test_agent");
}

#[test]
fn trait_model_name_callable() {
    let agent: Box<dyn Agent> = Box::new(MockAgent::new("agent"));
    // MockAgent default model_name returns "mock-model"
    assert_eq!(agent.model_name(), "mock-model");
}

#[test]
fn trait_tool_names_default() {
    let agent: Box<dyn Agent> = Box::new(MockAgent::new("agent"));

    assert!(agent.tool_names().is_empty());
}

#[test]
fn trait_skill_names_default() {
    let agent: Box<dyn Agent> = Box::new(MockAgent::new("agent"));

    assert!(agent.skill_names().is_empty());
}

// ── MockAgent contract ─────────────────────────────────────────────────────────

/// chat() should record calls and consume the preset response queue
#[tokio::test]
async fn mock_agent_chat_records_calls_and_consumes_responses() {
    let agent = MockAgent::new("test")
        .with_response("Response1")
        .with_response("Response2")
        .with_response("Response3");

    let r1 = agent.chat("Message1").await.unwrap();
    let r2 = agent.chat("Message2").await.unwrap();
    let r3 = agent.chat("Message3").await.unwrap();

    assert_eq!(r1, "Response1");
    assert_eq!(r2, "Response2");
    assert_eq!(r3, "Response3");
    assert_eq!(agent.call_count(), 3);
    assert_eq!(agent.calls(), vec!["Message1", "Message2", "Message3"]);
}

/// reset() should clear MockAgent's call history (simulating conversation reset semantics)
#[tokio::test]
async fn mock_agent_reset_clears_call_history() {
    let agent = MockAgent::new("test")
        .with_response("r1")
        .with_response("r2")
        .with_response("r3");

    agent.chat("Round1 Message1").await.unwrap();
    agent.chat("Round1 Message2").await.unwrap();
    assert_eq!(agent.call_count(), 2, "before reset should have 2 records");

    agent.reset().await;
    assert_eq!(
        agent.call_count(),
        0,
        "after reset, call history should be cleared"
    );

    agent.chat("Round2 Message1").await.unwrap();
    assert_eq!(
        agent.call_count(),
        1,
        "after reset, round 2 should start counting from 1"
    );
    assert_eq!(agent.calls(), vec!["Round2 Message1"]);
}

/// execute() and chat() share the same response queue
#[tokio::test]
async fn mock_agent_execute_and_chat_share_response_queue() {
    let agent = MockAgent::new("test")
        .with_response("executeResponse")
        .with_response("chatResponse");

    let r1 = agent.execute("Task").await.unwrap();
    let r2 = agent.chat("Chat").await.unwrap();

    assert_eq!(r1, "executeResponse");
    assert_eq!(r2, "chatResponse");
    assert_eq!(agent.call_count(), 2);
}

/// Callers can explicitly opt into a reusable default response.
#[tokio::test]
async fn mock_agent_chat_falls_back_to_default_when_queue_empty() {
    let agent = MockAgent::new("test").with_default_success("mock agent response");

    let r = agent.chat("Any message").await.unwrap();
    assert_eq!(
        r, "mock agent response",
        "when queue is empty, should return default response"
    );
}

/// FailingMockAgent::reset() clears call history
#[tokio::test]
async fn failing_mock_agent_reset_clears_calls() {
    let agent = FailingMockAgent::new("failing", "Always fails");

    agent.execute("Task1").await.unwrap_err();
    agent.chat("Task2").await.unwrap_err();
    assert_eq!(agent.call_count(), 2);

    agent.reset().await;
    assert_eq!(
        agent.call_count(),
        0,
        "after reset, call records should be cleared"
    );
}

// ── chat + reset full lifecycle ────────────────────────────────────────────────

/// Simulate a typical multi-turn conversation lifecycle: chat → reset → chat
#[tokio::test]
async fn mock_agent_full_chat_lifecycle() {
    let agent = MockAgent::new("assistant").with_responses([
        "Round1Reply1",
        "Round1Reply2",
        "Round2Reply1",
        "Round2Reply2",
    ]);

    agent.chat("Round 1: Question A").await.unwrap();
    agent.chat("Round 1: Question B").await.unwrap();
    assert_eq!(agent.call_count(), 2);

    agent.reset().await;
    assert_eq!(agent.call_count(), 0);

    agent.chat("Round 2: Question C").await.unwrap();
    agent.chat("Round 2: Question D").await.unwrap();
    assert_eq!(agent.call_count(), 2);
    assert_eq!(
        agent.calls(),
        vec!["Round 2: Question C", "Round 2: Question D"]
    );
}

// ── ReactAgentBuilder Tests ───────────────────────────────────────────────────────

#[test]
fn react_agent_builder_basic() {
    let agent = crate::agent::ReactAgentBuilder::new()
        .name("test")
        .model("qwen3-max")
        .system_prompt("Be helpful")
        .build()
        .unwrap();

    assert_eq!(agent.name(), "test");
    assert_eq!(agent.model_name(), "qwen3-max");
    assert_eq!(agent.system_prompt(), "Be helpful");
}

#[test]
fn react_agent_builder_with_tools() {
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("qwen3-max")
        .enable_tools()
        .tool(Box::new(MockTool::new("tool1")))
        .tool(Box::new(MockTool::new("tool2")))
        .build()
        .unwrap();

    assert!(agent.config().is_tool_enabled());
    // FinalAnswerTool + built-in tools (count depends on enabled features) + tool1 + tool2
    let names = agent.tool_names();
    assert!(
        names.contains(&String::from("tool1")),
        "Should contain tool1"
    );
    assert!(
        names.contains(&String::from("tool2")),
        "Should contain tool2"
    );
    assert!(
        names.contains(&String::from("final_answer")),
        "Should contain final_answer"
    );
    assert!(names.len() >= 3, "Should have at least 3 tools");
}

#[test]
fn react_agent_builder_with_memory() {
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("qwen3-max")
        .enable_memory()
        .build()
        .unwrap();

    assert!(agent.config().is_memory_enabled());
}

#[test]
fn react_agent_builder_with_planning() {
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("qwen3-max")
        .enable_tasks()
        .build()
        .unwrap();

    assert!(agent.config().is_task_enabled());
}

#[test]
fn react_agent_builder_max_iterations() {
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("qwen3-max")
        .max_iterations(50)
        .build()
        .unwrap();

    assert_eq!(agent.config().get_max_iterations(), 50);
}

#[test]
fn react_agent_builder_token_limit() {
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("qwen3-max")
        .token_limit(DEFAULT_TOKEN_LIMIT)
        .build()
        .unwrap();

    assert_eq!(agent.config().get_token_limit(), DEFAULT_TOKEN_LIMIT);
}

#[tokio::test]
async fn prepared_model_generation_is_inert_until_infallible_commit() -> crate::error::Result<()> {
    let agent = ReactAgent::new(AgentConfig::new(
        "original-model",
        "test_agent",
        "system prompt",
    ));
    let llm_config = crate::llm::LlmConfig::openai("test-key", "replacement-model");
    let client: Arc<dyn crate::llm::LlmClient> =
        Arc::new(crate::testing::MockLlmClient::new().with_model_name("replacement-model"));

    let handle = AgentHandle::new(agent);
    let context = handle.read(|agent| Arc::clone(&agent.memory.context)).await;
    assert_eq!(
        handle.read(|agent| agent.model_name().to_string()).await,
        "original-model"
    );
    let prepared = handle
        .prepare_model_generation(
            llm_config,
            Arc::clone(&client),
            Some(0.25),
            Some(4096),
            None,
            32_768,
            super::PreparedCriticUpdate::Preserve,
        )
        .await?;

    assert!(handle.try_write(|_| ()).is_none());
    assert!(context.try_lock().is_err());

    prepared.commit();

    let projection = handle
        .read(|agent| {
            (
                agent.model_name().to_string(),
                agent.config().get_temperature(),
                agent.config().get_max_tokens(),
                agent.config().get_token_limit(),
                agent.llm_config().map(|config| config.model.clone()),
                agent
                    .llm_client()
                    .map(|value| value.model_name().to_string()),
            )
        })
        .await;
    assert_eq!(projection.0, "replacement-model");
    assert_eq!(projection.1, Some(0.25));
    assert_eq!(projection.2, Some(4096));
    assert_eq!(projection.3, 32_768);
    assert_eq!(projection.4.as_deref(), Some("replacement-model"),);
    assert_eq!(projection.5.as_deref(), Some("replacement-model"));
    assert!(context.try_lock().is_ok());
    Ok(())
}

#[tokio::test]
async fn mismatched_prepared_client_leaves_agent_unchanged() {
    let agent = AgentHandle::new(ReactAgent::new(AgentConfig::new(
        "original-model",
        "test_agent",
        "system prompt",
    )));
    let llm_config = crate::llm::LlmConfig::openai("test-key", "replacement-model");
    let client: Arc<dyn crate::llm::LlmClient> =
        Arc::new(crate::testing::MockLlmClient::new().with_model_name("wrong-model"));

    let result = agent
        .prepare_model_generation(
            llm_config,
            client,
            None,
            None,
            None,
            32_768,
            super::PreparedCriticUpdate::Preserve,
        )
        .await;

    assert!(result.is_err());
    assert_eq!(
        agent.read(|agent| agent.model_name().to_string()).await,
        "original-model"
    );
    assert_eq!(
        agent.read(|agent| agent.config().get_token_limit()).await,
        DEFAULT_TOKEN_LIMIT
    );
    assert!(agent.try_write(|_| ()).is_some());
}

#[tokio::test]
async fn owned_critic_refresh_does_not_replace_a_custom_critic() -> crate::error::Result<()> {
    let mut agent = ReactAgent::new(AgentConfig::new(
        "original-model",
        "test_agent",
        "system prompt",
    ));
    let custom = Arc::new(echo_core::agent::StaticCritic::always_pass());
    agent.set_critic(custom.clone());
    assert_eq!(agent.critic_owner(), None);
    let agent = AgentHandle::new(agent);

    let llm_config = crate::llm::LlmConfig::openai("test-key", "replacement-model");
    let client: Arc<dyn crate::llm::LlmClient> =
        Arc::new(crate::testing::MockLlmClient::new().with_model_name("replacement-model"));
    let replacement = Arc::new(echo_core::agent::StaticCritic::always_fail());
    let prepared = agent
        .prepare_model_generation(
            llm_config,
            client,
            None,
            None,
            None,
            32_768,
            super::PreparedCriticUpdate::ReplaceOwned {
                owner: "eko:model-generation".to_string(),
                critic: replacement,
            },
        )
        .await?;

    prepared.commit();

    assert_eq!(Arc::strong_count(&custom), 2);
    assert_eq!(
        agent
            .read(|agent| agent.critic_owner().map(str::to_string))
            .await,
        None
    );
    Ok(())
}

#[tokio::test]
async fn prepared_generation_is_type_bound_to_its_origin_agent() -> crate::error::Result<()> {
    let origin = AgentHandle::new(ReactAgent::new(AgentConfig::new(
        "origin-model",
        "origin",
        "system prompt",
    )));
    let other = AgentHandle::new(ReactAgent::new(AgentConfig::new(
        "other-model",
        "other",
        "system prompt",
    )));
    let client: Arc<dyn crate::llm::LlmClient> =
        Arc::new(crate::testing::MockLlmClient::new().with_model_name("replacement-model"));
    let prepared = origin
        .prepare_model_generation(
            crate::llm::LlmConfig::openai("test-key", "replacement-model"),
            client,
            None,
            None,
            None,
            32_768,
            super::PreparedCriticUpdate::Preserve,
        )
        .await?;

    assert!(origin.try_write(|_| ()).is_none());
    assert!(other.try_write(|_| ()).is_some());
    prepared.commit();

    assert_eq!(
        origin.read(|agent| agent.model_name().to_string()).await,
        "replacement-model"
    );
    assert_eq!(
        origin.read(|agent| agent.config().get_token_limit()).await,
        32_768
    );
    assert_eq!(
        other.read(|agent| agent.model_name().to_string()).await,
        "other-model"
    );
    assert_eq!(
        other.read(|agent| agent.config().get_token_limit()).await,
        DEFAULT_TOKEN_LIMIT
    );
    Ok(())
}

#[test]
fn react_agent_builder_session_id() {
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("qwen3-max")
        .session_id("session-123")
        .build()
        .unwrap();

    assert_eq!(agent.config().get_session_id(), Some("session-123"));
}

#[test]
fn react_agent_builder_conversation_id() {
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("qwen3-max")
        .conversation_id("conversation-123")
        .build()
        .unwrap();

    assert_eq!(
        agent.config().get_conversation_id(),
        Some("conversation-123")
    );
}

#[test]
fn react_agent_builder_split_thread_and_conversation_ids() {
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("qwen3-max")
        .session_id("thread-123")
        .conversation_id("conversation-123")
        .build()
        .unwrap();

    assert_eq!(agent.config().get_session_id(), Some("thread-123"));
    assert_eq!(
        agent.config().get_conversation_id(),
        Some("conversation-123")
    );
}

// ── ReactAgent Config Preset Tests ───────────────────────────────────────────────────────

#[test]
fn react_agent_builder_simple() {
    let agent = crate::agent::ReactAgentBuilder::simple("qwen3-max", "You are helpful").unwrap();

    assert_eq!(agent.model_name(), "qwen3-max");
    assert!(!agent.config().is_tool_enabled());
}

#[test]
fn react_agent_builder_standard() {
    let agent =
        crate::agent::ReactAgentBuilder::standard("qwen3-max", "agent1", "Be helpful").unwrap();

    assert!(agent.config().is_tool_enabled());
    assert!(agent.config().is_cot_enabled());
}

#[test]
fn react_agent_builder_full_featured() {
    let agent = crate::agent::ReactAgentBuilder::full_featured("qwen3-max", "agent1", "Be helpful")
        .unwrap();

    assert!(agent.config().is_tool_enabled());
    assert!(agent.config().is_memory_enabled());
    assert!(agent.config().is_task_enabled());
    assert!(agent.config().is_cot_enabled());
}

// ── SubAgent Tests ───────────────────────────────────────────────────────────────

#[cfg(feature = "subagent")]
#[test]
fn react_agent_register_subagent_requires_enable_flag() {
    // Do not enable subagent feature
    let config = AgentConfig::minimal("test-model", "main_agent");
    let mut agent = ReactAgent::new(config);

    let sub_agent = Box::new(MockAgent::new("sub_agent"));
    agent.register_agent(sub_agent);

    // Since enable_subagent = false, subagent should not be registered
    // There is no public method to directly check subagent list, but can verify through behavior
}

#[cfg(feature = "subagent")]
#[test]
fn react_agent_register_subagent_when_enabled() {
    let config = AgentConfig::minimal("test-model", "main_agent").enable_subagent(true);
    let mut agent = ReactAgent::new(config);

    let sub_agent = Box::new(MockAgent::new("sub_agent"));
    agent.register_agent(sub_agent);

    // subagent should be successfully registered
    // Can indirectly verify by checking if agent_dispatch tool is available
}

#[cfg(feature = "subagent")]
#[test]
fn react_agent_register_multiple_subagents() {
    let config = AgentConfig::minimal("test-model", "main_agent").enable_subagent(true);
    let mut agent = ReactAgent::new(config);

    let sub_agents: Vec<Box<dyn Agent>> = vec![
        Box::new(MockAgent::new("subagent1")),
        Box::new(MockAgent::new("subagent2")),
        Box::new(MockAgent::new("subagent3")),
    ];

    agent.register_agents(sub_agents);

    // All subagents should be successfully registered
}

#[tokio::test]
async fn subagent_context_isolation() {
    // Create parent agent
    let parent_config =
        AgentConfig::new("qwen3-max", "parent", "You are the parent agent").enable_subagent(true);
    let parent = ReactAgent::new(parent_config);

    // Parent agent adds message to context
    parent
        .memory
        .context
        .lock()
        .await
        .push(Message::user("Parent message".to_string()));
    let (parent_count_before, _) = parent.context_stats().await;
    assert_eq!(parent_count_before, 2); // system + user message

    // Create independent child agent
    let sub_config = AgentConfig::new("qwen3-max", "child", "You are a child agent");
    let child = ReactAgent::new(sub_config);

    assert!(!Arc::ptr_eq(
        &parent.execution_mutex,
        &child.execution_mutex
    ));
    assert!(!Arc::ptr_eq(&parent.memory.context, &child.memory.context));

    // Child agent has its own independent context
    let (child_count, _) = child.context_stats().await;
    assert_eq!(child_count, 1); // only system message

    // Child agent adding messages does not affect parent agent
    child
        .memory
        .context
        .lock()
        .await
        .push(Message::user("Child message".to_string()));
    let (child_count_after, _) = child.context_stats().await;
    assert_eq!(child_count_after, 2);

    // Parent agent's context is unaffected
    let (parent_count_after, _) = parent.context_stats().await;
    assert_eq!(parent_count_after, 2);
}

#[tokio::test]
async fn subagent_reset_independence() {
    // Create parent agent and child agent
    let parent_config =
        AgentConfig::new("qwen3-max", "parent", "Parent system").enable_subagent(true);
    let parent = ReactAgent::new(parent_config);

    let child_config = AgentConfig::new("qwen3-max", "child", "Child system");
    let child = ReactAgent::new(child_config);

    // Both add messages
    parent
        .memory
        .context
        .lock()
        .await
        .push(Message::user("Parent msg".to_string()));
    child
        .memory
        .context
        .lock()
        .await
        .push(Message::user("Child msg".to_string()));

    // Reset parent agent
    parent.reset().await;

    // Parent agent context is cleared
    let (parent_count, _) = parent.context_stats().await;
    assert_eq!(parent_count, 1);

    // Child agent context is unaffected
    let (child_count, _) = child.context_stats().await;
    assert_eq!(child_count, 2);
}

#[cfg(feature = "subagent")]
#[test]
fn react_agent_register_agent_dispatch_tool() {
    let config = AgentConfig::minimal("test-model", "main_agent")
        .enable_subagent(true)
        .register_agent_dispatch_tool(true);
    let agent = ReactAgent::new(config);

    // When register_agent_dispatch_tool is enabled, agent_tool should be registered
    let tool_names = agent.tool_names();
    assert!(tool_names.contains(&String::from("agent_tool")));
}

#[cfg(feature = "subagent")]
#[tokio::test]
async fn shared_subagent_registry_exposes_parent_registrations_to_child() -> crate::error::Result<()>
{
    let registry = Arc::new(SubagentRegistry::new());
    let mut parent = ReactAgentBuilder::new()
        .model("test-model")
        .name("parent")
        .enable_tools()
        .enable_subagent()
        .subagent_registry(registry.clone())
        .build()?;
    let child = ReactAgentBuilder::new()
        .model("test-model")
        .name("child")
        .enable_tools()
        .enable_subagent()
        .subagent_registry(registry.clone())
        .register_agent_dispatch_tool()
        .build()?;

    parent.register_subagent_with_definition(
        SubagentBuilder::new("reviewer")
            .description("Review code")
            .build(),
        Box::new(MockAgent::new("reviewer")),
    );

    assert!(Arc::ptr_eq(
        parent.subagent_registry(),
        child.subagent_registry()
    ));
    assert!(
        child
            .subagent_registry()
            .list_available()
            .await
            .iter()
            .any(|definition| definition.name == "reviewer")
    );
    assert!(child.tool_names().iter().any(|name| name == "agent_tool"));
    let definitions = child.subagent_registry().list_available().await;
    child.sync_subagent_dispatch_catalog(&definitions);
    let dispatch_schema = <ReactAgent as Agent>::tool_definitions(&child)
        .into_iter()
        .find(|definition| definition.function.name == "agent_tool")
        .and_then(|definition| {
            definition
                .function
                .parameters
                .pointer("/properties/agent_name/enum")
                .cloned()
        });
    assert!(
        dispatch_schema.is_some_and(|schema| {
            schema
                .as_array()
                .is_some_and(|names| names.iter().any(|name| name == "reviewer"))
        }),
        "shared targets must be discoverable through the child agent_tool schema"
    );
    Ok(())
}

#[cfg(feature = "subagent")]
#[test]
fn agent_dispatch_tool_schema_lists_registered_subagents() {
    let config = AgentConfig::minimal("test-model", "main_agent")
        .enable_subagent(true)
        .register_agent_dispatch_tool(true);
    let mut agent = ReactAgent::new(config);

    let def = SubagentBuilder::new("code_reviewer")
        .description("Reviews code for bugs and test gaps")
        .fork_mode()
        .tag("readonly")
        .build();
    agent.register_subagent_with_definition(def, Box::new(MockAgent::new("code_reviewer")));

    let definitions = <ReactAgent as Agent>::tool_definitions(&agent);
    let maybe_dispatch = definitions
        .iter()
        .find(|definition| definition.function.name == "agent_tool");
    let Some(dispatch) = maybe_dispatch else {
        assert!(
            maybe_dispatch.is_some(),
            "agent_tool should be registered when subagent is enabled"
        );
        return;
    };

    let agent_name_schema = dispatch
        .function
        .parameters
        .get("properties")
        .and_then(|properties| properties.get("agent_name"));
    let Some(agent_name_schema) = agent_name_schema else {
        assert!(
            agent_name_schema.is_some(),
            "agent_name schema should exist"
        );
        return;
    };

    let enum_values = agent_name_schema
        .get("enum")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(enum_values.iter().any(|value| value == "code_reviewer"));

    let description = agent_name_schema
        .get("description")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    assert!(description.contains("Reviews code for bugs and test gaps"));

    let task_description = dispatch
        .function
        .parameters
        .pointer("/properties/task/description")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    assert!(task_description.contains("user's current language"));
}

#[cfg(feature = "subagent")]
#[test]
fn react_agent_no_agent_dispatch_without_subagent() {
    let config = AgentConfig::minimal("test-model", "main_agent").enable_subagent(false);
    let agent = ReactAgent::new(config);

    // When subagent is not enabled, agent_tool should not be registered
    let tool_names = agent.tool_names();
    assert!(!tool_names.contains(&String::from("agent_tool")));
}

// ── Agent Config Isolation Tests ───────────────────────────────────────────────────────

#[test]
fn agent_config_isolation() {
    // Create two agents with independent configurations
    let config1 = AgentConfig::new("model-a", "agent1", "System A");
    let config2 = AgentConfig::new("model-b", "agent2", "System B");

    let agent1 = ReactAgent::new(config1);
    let agent2 = ReactAgent::new(config2);

    // Verify configurations are completely independent
    assert_eq!(agent1.model_name(), "model-a");
    assert_eq!(agent2.model_name(), "model-b");
    assert_eq!(agent1.name(), "agent1");
    assert_eq!(agent2.name(), "agent2");
    assert_eq!(agent1.system_prompt(), "System A");
    assert_eq!(agent2.system_prompt(), "System B");
}

#[test]
fn agent_tool_registration_isolation() {
    let config1 = AgentConfig::minimal("model", "agent1");
    let config2 = AgentConfig::minimal("model", "agent2");

    let mut agent1 = ReactAgent::new(config1);
    let agent2 = ReactAgent::new(config2);

    // agent1 registers a tool
    agent1.add_tool(Box::new(MockTool::new("tool1")));

    // agent2 should not be affected
    let tools1 = agent1.tool_names();
    let tools2 = agent2.tool_names();

    // agent1 has built-in tools + tool1; agent2 only has built-in tools
    assert!(
        tools1.len() >= 2,
        "agent1 should have at least 2 tools (built-in + tool1)"
    );
    assert!(
        !tools2.is_empty(),
        "agent2 should have at least 1 built-in tool"
    );
    assert_eq!(
        tools1.len(),
        tools2.len() + 1,
        "agent1 should have exactly one more tool than agent2"
    );
}

#[test]
fn agent_callbacks_isolation() {
    let config1 = AgentConfig::minimal("model", "agent1");
    let config2 = AgentConfig::minimal("model", "agent2");

    let mut agent1 = ReactAgent::new(config1);
    let agent2 = ReactAgent::new(config2);

    // agent1 adds a callback
    let callback = Arc::new(CounterCallback::new());
    agent1.add_callback(callback);

    // agent2 should not be affected (verified through execution behavior)
    // Since callbacks are private, only verify the method doesn't panic
    let _ = agent2;
}

// ── Agent Human-in-Loop Tool Tests ───────────────────────────────────────────────

#[cfg(feature = "human-loop")]
#[test]
fn react_agent_human_in_loop_tool_registration() {
    let config = AgentConfig::minimal("model", "agent").enable_human_in_loop(true);
    let agent = ReactAgent::new(config);

    // After enabling human_in_loop, the human_in_loop tool should be registered
    let tool_names = agent.tool_names();
    assert!(tool_names.contains(&String::from("human_in_loop")));
}

#[cfg(feature = "human-loop")]
#[test]
fn react_agent_no_human_in_loop_without_flag() {
    let config = AgentConfig::minimal("model", "agent").enable_human_in_loop(false);
    let agent = ReactAgent::new(config);

    // When human_in_loop is not enabled, the tool should not be registered
    let tool_names = agent.tool_names();
    assert!(!tool_names.contains(&String::from("human_in_loop")));
}

#[tokio::test]
#[cfg(feature = "human-loop")]
async fn add_need_appeal_tool_does_not_nest_runtime_with_permission_service() {
    struct AllowProvider;
    impl crate::human_loop::HumanLoopProvider for AllowProvider {
        fn request(
            &self,
            _req: crate::human_loop::HumanLoopRequest,
        ) -> futures::future::BoxFuture<
            '_,
            crate::error::Result<crate::human_loop::HumanLoopResponse>,
        > {
            Box::pin(async { Ok(crate::human_loop::HumanLoopResponse::Approved) })
        }
    }

    let provider = Arc::new(AllowProvider);
    let service = Arc::new(crate::human_loop::PermissionService::from_provider(
        provider.clone() as Arc<dyn crate::human_loop::HumanLoopProvider>,
    ));

    let config = AgentConfig::minimal("model", "agent").enable_human_in_loop(true);
    let mut agent = ReactAgent::new(config);
    agent.set_permission_service(service.clone());

    // Before fix, this would trigger Handle::current().block_on(...) panic in async context.
    agent.add_need_appeal_tool(Box::new(MockTool::new("dangerous_tool")));

    let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
    let _ = snapshot
        .check_tool_approval("call-1", "dangerous_tool", &serde_json::json!({}), None)
        .await;

    let rules = service.all_rules().await;
    assert!(rules.iter().any(|rule| {
        matches!(
            &rule.matcher,
            echo_core::tools::permission::RuleMatcher::Pattern { pattern }
                if pattern == "dangerous_tool"
        )
    }));
}

#[tokio::test]
async fn discover_skills_refreshes_activate_skill_registry() {
    let base =
        std::env::temp_dir().join(format!("echo-agent-skill-refresh-{}", std::process::id()));
    let dir1 = base.join("skills-a").join("skill-one");
    let dir2 = base.join("skills-b").join("skill-two");
    tokio::fs::create_dir_all(&dir1).await.unwrap();
    tokio::fs::create_dir_all(&dir2).await.unwrap();

    tokio::fs::write(
        dir1.join("SKILL.md"),
        "---\nname: skill-one\ndescription: first skill\n---\n\nUse skill one.\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        dir2.join("SKILL.md"),
        "---\nname: skill-two\ndescription: second skill\n---\n\nUse skill two.\n",
    )
    .await
    .unwrap();

    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);
    let base_system_prompt = agent.config.system_prompt.clone();

    agent
        .discover_skills(&[DiscoveryScope::Custom(base.join("skills-a"))])
        .await
        .unwrap();

    let first_params = agent
        .tools
        .tool_manager
        .get_tool("activate_skill")
        .expect("activate_skill should be registered")
        .parameters()
        .to_string();
    assert!(first_params.contains(&String::from("skill-one")));
    assert!(!first_params.contains(&String::from("skill-two")));

    let first_activation = agent
        .tools
        .tool_manager
        .execute_tool(
            "activate_skill",
            [("name".to_string(), json!("skill-one"))].into(),
        )
        .await
        .unwrap();
    assert!(first_activation.success);
    assert!(
        first_activation
            .output
            .contains(&String::from("Use skill one."))
    );

    agent
        .discover_skills(&[DiscoveryScope::Custom(base.join("skills-b"))])
        .await
        .unwrap();

    assert_eq!(agent.config.system_prompt, base_system_prompt);
    {
        let context = agent.memory.context.lock().await;
        assert!(context.has_projection("echo-agent:skill-catalog"));
        let catalogs: Vec<_> = context
            .messages()
            .iter()
            .filter(|message| {
                message.content.as_text_ref().is_some_and(|text| {
                    text.contains("The following skills") && text.contains("skill-one")
                })
            })
            .collect();
        assert_eq!(catalogs.len(), 1);
        assert!(catalogs.first().is_some_and(|message| {
            message
                .content
                .as_text_ref()
                .is_some_and(|text| text.contains("skill-two"))
        }));
    }

    let second_params = agent
        .tools
        .tool_manager
        .get_tool("activate_skill")
        .expect("activate_skill should stay registered")
        .parameters()
        .to_string();
    assert!(second_params.contains(&String::from("skill-one")));
    assert!(second_params.contains(&String::from("skill-two")));

    let repeat_activation = agent
        .tools
        .tool_manager
        .execute_tool(
            "activate_skill",
            [("name".to_string(), json!("skill-one"))].into(),
        )
        .await
        .unwrap();
    assert!(repeat_activation.success);
    assert!(
        repeat_activation
            .output
            .contains(&String::from("already activated"))
    );

    let second_activation = agent
        .tools
        .tool_manager
        .execute_tool(
            "activate_skill",
            [("name".to_string(), json!("skill-two"))].into(),
        )
        .await
        .unwrap();
    assert!(second_activation.success);
    assert!(
        second_activation
            .output
            .contains(&String::from("Use skill two."))
    );

    let _ = tokio::fs::remove_dir_all(base).await;
}

#[tokio::test]
async fn intent_router_skill_activation_survives_compression_markers() -> Result<(), String> {
    let base =
        std::env::temp_dir().join(format!("echo-agent-skill-protected-{}", std::process::id()));
    let skill_dir = base.join("skills").join("skill-protected");
    let _ = tokio::fs::remove_dir_all(&base).await;
    tokio::fs::create_dir_all(&skill_dir)
        .await
        .map_err(|error| format!("create skill dir: {error}"))?;
    tokio::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: skill-protected\ndescription: protected skill\n---\n\nProtected instructions survive compression.\n",
    )
    .await
    .map_err(|error| format!("write skill file: {error}"))?;

    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);
    agent
        .discover_skills(&[DiscoveryScope::Custom(base.join("skills"))])
        .await
        .map_err(|error| format!("discover skill: {error}"))?;
    agent
        .activate_skill("skill-protected")
        .await
        .map_err(|error| format!("activate skill: {error}"))?;

    {
        let mut ctx = agent.memory.context.lock().await;
        ctx.push(Message::user("old user message".to_string()));
        ctx.push(Message::assistant("old assistant message".to_string()));
        ctx.push(Message::user("latest user message".to_string()));
    }

    let compressor = crate::compression::compressor::SlidingWindowCompressor::new(1);
    agent
        .force_compress_with(&compressor)
        .await
        .map_err(|error| format!("force compress: {error}"))?;

    let messages = agent.memory.context.lock().await.messages().to_vec();
    assert!(
        messages.iter().any(|message| {
            message
                .content
                .as_text_ref()
                .is_some_and(|text| text.contains("<skill_content"))
        }),
        "activated skill block should retain the protected marker after compression"
    );
    assert!(
        messages.iter().any(|message| {
            message
                .content
                .as_text_ref()
                .is_some_and(|text| text.contains("Protected instructions survive compression."))
        }),
        "activated skill instructions should remain after compression"
    );

    let _ = tokio::fs::remove_dir_all(&base).await;
    Ok(())
}

#[tokio::test]
async fn plugin_skill_variables_cover_frontmatter_hooks_and_body() -> Result<(), String> {
    let base = std::env::temp_dir().join(format!(
        "echo-agent-plugin-skill-variables-{}",
        uuid::Uuid::new_v4()
    ));
    let skill_dir = base.join("configured-skill");
    tokio::fs::create_dir_all(&skill_dir)
        .await
        .map_err(|error| format!("create skill dir: {error}"))?;
    tokio::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: configured-skill\ndescription: Configured skill\nhooks:\n  PreToolUse:\n    - matcher: Bash\n      hooks:\n        - type: command\n          command: notify ${user_config.endpoint}\n---\nUse ${user_config.endpoint}.\n",
    )
    .await
    .map_err(|error| format!("write skill file: {error}"))?;

    let variables = crate::plugin::PluginVariables::new(
        "configured-plugin",
        base.clone(),
        base.join("project"),
    )
    .with_user_config(std::collections::HashMap::from([(
        "endpoint".to_string(),
        "http://localhost:9100".to_string(),
    )]));
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);
    agent
        .load_plugin_skills_from_dir(&base, "plugin:configured-plugin", &variables)
        .await
        .map_err(|error| format!("load plugin skill: {error}"))?;

    let descriptor = agent
        .skill_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.name == "configured-skill")
        .ok_or_else(|| "configured plugin skill was not registered".to_string())?;
    let action = descriptor
        .hooks
        .as_ref()
        .and_then(|definition| definition.rules_for(HookEvent::PreToolUse).first())
        .and_then(|rule| rule.hooks.first())
        .ok_or_else(|| "configured frontmatter hook was not registered".to_string())?;
    match action {
        HookAction::Command { command, .. } => {
            assert_eq!(command, "notify http://localhost:9100");
        }
        _ => return Err("configured hook is not a command".to_string()),
    }

    agent
        .activate_skill("configured-skill")
        .await
        .map_err(|error| format!("activate plugin skill: {error}"))?;
    assert!(agent.get_messages().await.iter().any(|message| {
        message
            .content
            .as_text_ref()
            .is_some_and(|text| text.contains("Use http://localhost:9100."))
    }));

    let _ = tokio::fs::remove_dir_all(base).await;
    Ok(())
}

#[tokio::test]
async fn execute_tool_injects_pre_and_post_hook_messages_into_context() -> crate::error::Result<()>
{
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(
        MockTool::new("test_tool").with_response("tool ok"),
    ));

    let mut hooks = agent.tools.hook_registry.write().await;
    let mut hook_def = HooksDefinition::default();
    hook_def.add_rules(
        HookEvent::PreToolUse,
        vec![HookRule {
            matcher: "test_tool".into(),
            hooks: vec![HookAction::Prompt {
                prompt: "pre-hook guidance".into(),
            }],
        }],
    );
    hook_def.add_rules(
        HookEvent::PostToolUse,
        vec![HookRule {
            matcher: "test_tool".into(),
            hooks: vec![HookAction::Prompt {
                prompt: "post-hook guidance".into(),
            }],
        }],
    );
    hooks.register("hook-skill", "/tmp", hook_def);
    drop(hooks);

    let input = json!({});
    let result = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent)
        .execute_tool_with_policy(
            "hook-test".to_string(),
            "test_tool",
            &crate::tools::ToolParameters::new(),
            &input,
            None,
        )
        .await;
    let result = match result {
        Ok(output) => output,
        Err(failure) => return Err(failure.error),
    };
    assert_eq!(result, "tool ok");

    let messages: Vec<String> = agent
        .get_messages()
        .await
        .iter()
        .filter_map(|m| m.content.as_text_ref().map(str::to_string))
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains(&String::from("pre-hook guidance"))),
        "pre-hook context missing from {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains(&String::from("post-hook guidance")))
    );
    Ok(())
}

#[cfg(feature = "shell")]
#[tokio::test]
async fn shell_skill_uses_agent_sandbox_manager_when_present() -> crate::error::Result<()> {
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);
    agent.set_sandbox_manager(Arc::new(SandboxManager::local_only()));
    agent.add_skill(Box::new(ShellSkill::new()));

    let input = json!({"command": "echo sandboxed"});
    let params = match input.clone() {
        serde_json::Value::Object(values) => values.into_iter().collect(),
        _ => crate::tools::ToolParameters::new(),
    };
    let result = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent)
        .execute_tool_with_policy("shell-test".to_string(), "shell", &params, &input, None)
        .await;
    match result {
        Ok(output) => assert!(output.contains(&String::from("sandboxed"))),
        Err(failure) => return Err(failure.error),
    }
    Ok(())
}

#[tokio::test]
async fn activate_skill_enforces_context_path_for_conditional_skills() {
    let base = std::env::temp_dir().join(format!(
        "echo-agent-conditional-skill-{}",
        std::process::id()
    ));
    let skill_dir = base.join("python-linter");
    tokio::fs::create_dir_all(&skill_dir).await.unwrap();
    tokio::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: python-linter\ndescription: Lint Python files\npaths:\n  - \"*.py\"\n---\n\nLint the current Python file.\n",
    )
    .await
    .unwrap();

    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);
    agent
        .discover_skills(&[DiscoveryScope::Custom(base.clone())])
        .await
        .unwrap();

    let missing = agent
        .tools
        .tool_manager
        .execute_tool(
            "activate_skill",
            [("name".to_string(), json!("python-linter"))].into(),
        )
        .await
        .unwrap();
    assert!(!missing.success);
    assert!(
        missing
            .error
            .unwrap_or_default()
            .contains(&String::from("context_path"))
    );

    let mismatch = agent
        .tools
        .tool_manager
        .execute_tool(
            "activate_skill",
            [
                ("name".to_string(), json!("python-linter")),
                ("context_path".to_string(), json!("src/main.rs")),
            ]
            .into(),
        )
        .await
        .unwrap();
    assert!(!mismatch.success);
    assert!(
        mismatch
            .error
            .unwrap_or_default()
            .contains(&String::from("cannot be activated"))
    );

    let matched = agent
        .tools
        .tool_manager
        .execute_tool(
            "activate_skill",
            [
                ("name".to_string(), json!("python-linter")),
                ("context_path".to_string(), json!("app.py")),
            ]
            .into(),
        )
        .await
        .unwrap();
    assert!(matched.success);
    assert!(
        matched
            .output
            .contains(&String::from("Lint the current Python file."))
    );

    let _ = tokio::fs::remove_dir_all(base).await;
}

// ── Agent Task Planning Tool Tests ───────────────────────────────────────────────────────

#[test]
#[cfg(feature = "tasks")]
fn react_agent_planning_tools_registration() {
    let config = AgentConfig::minimal("model", "agent").enable_task(true);
    let agent = ReactAgent::new(config);

    // The background-task tools are registered when `enable_task` is set.
    // (Tool names track the current SpawnBackgroundTaskTool / CheckTaskStatusTool
    // / ListBackgroundTasksTool implementations — older names like create_task
    // / update_task / list_tasks were renamed long ago.)
    let tool_names = agent.tool_names();
    assert!(tool_names.contains(&String::from("spawn_background_task")));
    assert!(tool_names.contains(&String::from("check_task_status")));
    assert!(tool_names.contains(&String::from("list_background_tasks")));
}

#[test]
fn react_agent_no_planning_tools_without_flag() {
    let config = AgentConfig::minimal("model", "agent").enable_task(false);
    let agent = ReactAgent::new(config);

    let tool_names = agent.tool_names();
    // When task planning is not enabled, related tools should not be available
    assert!(!tool_names.contains(&String::from("create_task")));
}

// ══════════════════════════════════════════════════════════════════════════════
// Feature 1: Memory Tool auto-injection (with_memory_tools + SearchMemoryTool)
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn builder_with_memory_tools_registers_all_memory_tools() {
    let store = Arc::new(crate::memory::store::InMemoryStore::new());
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("qwen3-max")
        .with_memory_tools(store)
        .build()
        .unwrap();

    let tools = agent.tool_names();
    assert!(
        tools.contains(&String::from("remember")),
        "Should register remember"
    );
    assert!(
        tools.contains(&String::from("recall")),
        "Should register recall"
    );
    assert!(
        tools.contains(&String::from("search_memory")),
        "Should register search_memory"
    );
    assert!(
        tools.contains(&String::from("forget")),
        "Should register forget"
    );
}

#[test]
fn builder_with_memory_tools_sets_store() {
    let store = Arc::new(crate::memory::store::InMemoryStore::new());
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("qwen3-max")
        .with_memory_tools(store)
        .build()
        .unwrap();

    assert!(agent.store().is_some(), "Store should be set");
}

#[test]
fn set_memory_store_registers_search_memory_tool() {
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);

    assert!(
        !agent.tool_names().contains(&String::from("search_memory")),
        "Should not have search_memory initially"
    );

    let store = Arc::new(crate::memory::store::InMemoryStore::new());
    agent.set_memory_store(store);

    assert!(
        agent.tool_names().contains(&String::from("search_memory")),
        "Should have search_memory after set_memory_store"
    );
}

#[tokio::test]
async fn search_memory_tool_returns_empty_for_no_matches() {
    let store = Arc::new(crate::memory::store::InMemoryStore::new());
    let tool = crate::tools::builtin::memory::SearchMemoryTool::new(
        store,
        vec!["test".to_string(), "memories".to_string()],
    );
    use crate::tools::Tool;
    let mut params = std::collections::HashMap::new();
    params.insert(
        "query".to_string(),
        serde_json::Value::String("Non-existent memory".to_string()),
    );
    let result = tool.execute(params).await.unwrap();
    assert!(result.success);
    assert!(result.output.contains(&String::from("No memories found")));
}

// ══════════════════════════════════════════════════════════════════════════════
// Feature 2: Token budget management (max_tool_output_tokens)
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn config_max_tool_output_tokens_default_is_none() {
    let config = AgentConfig::new("model", "agent", "prompt");
    assert_eq!(config.get_max_tool_output_tokens(), None);
}

#[test]
fn config_max_tool_output_tokens_setter() {
    let config = AgentConfig::new("model", "agent", "prompt").max_tool_output_tokens(2000);
    assert_eq!(config.get_max_tool_output_tokens(), Some(2000));
}

#[test]
fn builder_max_tool_output_tokens() {
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("qwen3-max")
        .max_tool_output_tokens(1500)
        .build()
        .unwrap();

    assert_eq!(agent.config().get_max_tool_output_tokens(), Some(1500));
}

#[tokio::test]
async fn truncate_tool_output_no_limit() {
    let config = AgentConfig::new("model", "agent", "prompt");
    let agent = ReactAgent::new(config);
    let long_text = "a".repeat(10000);
    let result = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent)
        .truncate_tool_output(long_text.clone())
        .await;
    assert_eq!(
        result.len(),
        long_text.len(),
        "Should not truncate when no limit is set"
    );
}

#[tokio::test]
async fn truncate_tool_output_within_limit() {
    let config = AgentConfig::new("model", "agent", "prompt").max_tool_output_tokens(100000);
    let agent = ReactAgent::new(config);
    let short_text = "hello world".to_string();
    let result = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent)
        .truncate_tool_output(short_text.clone())
        .await;
    assert_eq!(result, short_text, "Should not truncate when within limit");
}

#[tokio::test]
async fn truncate_tool_output_exceeds_limit() {
    let config = AgentConfig::new("model", "agent", "prompt")
        .max_tool_output_tokens(10)
        .tool_output_artifacts(None);
    let agent = ReactAgent::new(config);
    let long_text = "a ".repeat(500);
    let result = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent)
        .truncate_tool_output(long_text)
        .await;
    assert!(
        result.contains(&String::from("[Output truncated")),
        "Should show truncation notice when over limit"
    );
    assert!(
        result.len() < 1000,
        "Should be significantly shorter after truncation"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// Feature 5: Dynamic Tool Registration/Deregistration
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn remove_tool_basic() {
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(MockTool::new("tool_a")));
    agent.add_tool(Box::new(MockTool::new("tool_b")));

    assert!(agent.tool_names().contains(&String::from("tool_a")));

    let removed = agent.remove_tool("tool_a");
    assert!(removed, "Should return true for successful removal");
    assert!(
        !agent.tool_names().contains(&String::from("tool_a")),
        "Should not exist after removal"
    );
    assert!(
        agent.tool_names().contains(&String::from("tool_b")),
        "Other tools should be unaffected"
    );
}

#[test]
fn remove_tool_nonexistent() {
    let config = AgentConfig::minimal("model", "agent");
    let agent = ReactAgent::new(config);
    let removed = agent.remove_tool("nonexistent");
    assert!(!removed, "Should return false for nonexistent tool");
}

#[test]
fn replace_tool_basic() {
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(MockTool::new("tool_x")));

    let old = agent.replace_tool(Box::new(MockTool::new("tool_x")));
    assert!(old.is_some(), "Should return the old tool");
    assert!(
        agent.tool_names().contains(&String::from("tool_x")),
        "New tool should exist"
    );
}

#[test]
fn replace_tool_when_not_exists() {
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);

    let old = agent.replace_tool(Box::new(MockTool::new("new_tool")));
    assert!(
        old.is_none(),
        "Should return None when old tool doesn't exist"
    );
    assert!(
        agent.tool_names().contains(&String::from("new_tool")),
        "New tool should be registered"
    );
}

#[tokio::test]
async fn invocation_history_is_inserted_before_current_input() -> Result<(), String> {
    use crate::agent::react::run::types::StreamMode;

    let config = AgentConfig::new("test-model", "history-order", "system prompt");
    let agent = ReactAgent::new(config);
    let history = vec![
        Message::user("inherited user".to_string()),
        Message::assistant("inherited assistant".to_string()),
    ];

    agent
        .prepare_stream_context(StreamMode::Chat, "current input", &history)
        .await
        .map_err(|error| error.to_string())?;

    let messages = agent.memory.context.lock().await.messages().to_vec();
    let position = |needle: &str| {
        messages.iter().position(|message| {
            message
                .text_content()
                .is_some_and(|content| content == needle)
        })
    };
    let system = position("system prompt").ok_or_else(|| "system prompt missing".to_string())?;
    let inherited_user =
        position("inherited user").ok_or_else(|| "inherited user missing".to_string())?;
    let inherited_assistant =
        position("inherited assistant").ok_or_else(|| "inherited assistant missing".to_string())?;
    let current = position("current input").ok_or_else(|| "current input missing".to_string())?;

    assert!(system < inherited_user);
    assert!(inherited_user < inherited_assistant);
    assert!(inherited_assistant < current);
    Ok(())
}

#[tokio::test]
async fn corrupt_runtime_checkpoint_blocks_restore_without_overwrite() -> Result<(), String> {
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let checkpoint_dir = temp
        .path()
        .join("runtime_state")
        .join("corrupt-conversation");
    std::fs::create_dir_all(&checkpoint_dir).map_err(|error| error.to_string())?;
    let checkpoint_path = checkpoint_dir.join("checkpoint.json");
    let corrupt = b"{ truncated checkpoint";
    std::fs::write(&checkpoint_path, corrupt).map_err(|error| error.to_string())?;

    let config = AgentConfig::new("test-model", "restore-barrier", "system prompt")
        .conversation_id("corrupt-conversation");
    let mut agent = ReactAgent::new(config);
    let store =
        crate::state::FileRuntimeStateStore::new(temp.path()).map_err(|error| error.to_string())?;
    agent.set_state_store(Arc::new(store));
    agent
        .memory
        .context
        .lock()
        .await
        .push(Message::user("preserve me".to_string()));

    let result = agent.restore_thread_context().await;
    assert!(result.is_err());
    assert!(
        agent
            .memory
            .context
            .lock()
            .await
            .messages()
            .iter()
            .any(|message| {
                message
                    .text_content()
                    .is_some_and(|content| content == "preserve me")
            })
    );
    assert_eq!(
        std::fs::read(&checkpoint_path).map_err(|error| error.to_string())?,
        corrupt
    );
    Ok(())
}

#[tokio::test]
async fn cold_chat_restores_persisted_checkpoint() -> Result<(), String> {
    use crate::state::RuntimeStateStore;

    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let store = Arc::new(
        crate::state::FileRuntimeStateStore::new(temp.path()).map_err(|error| error.to_string())?,
    );
    let mut checkpoint = crate::state::AgentCheckpoint::new("cold-chat");
    checkpoint.messages_json = serde_json::to_string(&vec![
        Message::system("system prompt".to_string()),
        Message::user("persisted turn".to_string()),
    ])
    .map_err(|error| error.to_string())?;
    store
        .save_checkpoint(&checkpoint)
        .await
        .map_err(|error| error.to_string())?;

    let config = AgentConfig::new("test-model", "cold-chat-agent", "system prompt")
        .conversation_id("cold-chat");
    let mut agent = ReactAgent::new(config);
    agent.set_state_store(store);
    agent
        .restore_chat_context_if_cold()
        .await
        .map_err(|error| error.to_string())?;

    assert!(agent.get_messages().await.iter().any(|message| {
        message
            .text_content()
            .is_some_and(|content| content == "persisted turn")
    }));
    Ok(())
}

#[tokio::test]
async fn warm_chat_history_is_not_replaced_by_checkpoint() -> Result<(), String> {
    use crate::state::RuntimeStateStore;

    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let store = Arc::new(
        crate::state::FileRuntimeStateStore::new(temp.path()).map_err(|error| error.to_string())?,
    );
    let mut checkpoint = crate::state::AgentCheckpoint::new("warm-chat");
    checkpoint.messages_json = serde_json::to_string(&vec![
        Message::system("system prompt".to_string()),
        Message::user("stale persisted turn".to_string()),
    ])
    .map_err(|error| error.to_string())?;
    store
        .save_checkpoint(&checkpoint)
        .await
        .map_err(|error| error.to_string())?;

    let config = AgentConfig::new("test-model", "warm-chat-agent", "system prompt")
        .conversation_id("warm-chat");
    let mut agent = ReactAgent::new(config);
    agent.set_state_store(store);
    agent
        .memory
        .context
        .lock()
        .await
        .push(Message::user("live turn".to_string()));
    agent
        .restore_chat_context_if_cold()
        .await
        .map_err(|error| error.to_string())?;

    let messages = agent.get_messages().await;
    assert!(messages.iter().any(|message| {
        message
            .text_content()
            .is_some_and(|content| content == "live turn")
    }));
    assert!(!messages.iter().any(|message| {
        message
            .text_content()
            .is_some_and(|content| content == "stale persisted turn")
    }));
    Ok(())
}

// ── recall_long_term_memories injects into the current user turn ─────────────

/// Recalled memories are dynamic per turn, so they must not be appended as
/// permanent system messages where they destabilize the provider's prompt-cache
/// prefix. The current turn owns one replaceable projection before the request.
#[tokio::test]
async fn recall_injects_memories_into_current_user_message() -> Result<(), String> {
    use crate::agent::react::run::types::StreamMode;
    use crate::memory::InMemoryStore;
    use serde_json::json;

    let agent_name = "recall_role_test";
    let config = AgentConfig::new("test-model", agent_name, "system prompt").enable_memory(false);
    let mut agent = ReactAgent::new(config);

    // Seed the long-term store with a fact that will be recalled by exact-text search.
    // (stage4 A2) Seed the unified namespace ["agent","memories"] — recall no
    // longer reads the legacy per-agent namespace.
    let store: Arc<dyn crate::memory::Store> = Arc::new(InMemoryStore::new());
    store
        .put(
            &["agent", "memories"],
            "fact-1",
            json!({ "content": "user prefers Rust over Python", "importance": 0.9 }),
        )
        .await
        .map_err(|error| error.to_string())?;
    agent.set_memory_store(store);

    // Drive prepare_stream_context with a query that should hit the seeded fact.
    let recalled = agent
        .prepare_stream_context(StreamMode::Chat, "Rust", &[])
        .await
        .map_err(|error| error.to_string())?;
    assert!(
        recalled >= 1,
        "expected at least one memory to be recalled, got {recalled}"
    );

    // The context keeps stable workspace/runtime state before the current user
    // request so provider prefix caches can reuse the shared project context
    // across different requests.
    let ctx = agent.memory.context.lock().await;
    let messages = ctx.messages();

    let memory_systems: Vec<&Message> = messages
        .iter()
        .filter(|m| {
            m.role == Role::System
                && m.text_content()
                    .is_some_and(|c| c.contains("[memory_context]"))
        })
        .collect();
    assert_eq!(
        memory_systems.len(),
        0,
        "memory_context should not be injected as system message, found: {:?}",
        messages
            .iter()
            .map(|m| format!("{:?}: {:?}", m.role, m.text_content()))
            .collect::<Vec<_>>()
    );

    let memory_users: Vec<&Message> = messages
        .iter()
        .filter(|m| {
            m.role == Role::User
                && m.text_content()
                    .is_some_and(|c| c.contains("[memory_context]"))
        })
        .collect();
    assert_eq!(
        memory_users.len(),
        1,
        "expected exactly one user message with [memory_context] marker"
    );

    assert!(
        memory_users
            .first()
            .and_then(|message| message.text_content())
            .is_some_and(|c| c.contains("user prefers Rust over Python")),
        "memory_context user message should contain seeded fact"
    );
    assert!(
        !memory_users
            .first()
            .and_then(|message| message.text_content())
            .is_some_and(|c| c.contains("[current_user_request]") || c.contains("\nRust")),
        "memory_context user message must not duplicate the current request"
    );

    let context_then_request = messages.windows(2).any(|pair| {
        pair[0].role == Role::User
            && pair[0]
                .text_content()
                .is_some_and(|c| c.contains("[runtime_context:memory]"))
            && pair[1].role == Role::User
            && pair[1].text_content().is_some_and(|c| c == "Rust")
    });
    assert!(
        context_then_request,
        "expected runtime context before current request so shared workspace context remains prefix-cacheable"
    );

    drop(ctx);
    agent
        .prepare_stream_context(StreamMode::Chat, "unrelated-query", &[])
        .await
        .map_err(|error| error.to_string())?;
    let ctx = agent.memory.context.lock().await;
    let runtime_contexts: Vec<_> = ctx
        .messages()
        .iter()
        .filter(|message| {
            message
                .text_content()
                .is_some_and(|content| content.contains("[runtime_context:memory]"))
        })
        .collect();
    assert!(
        runtime_contexts.len() <= 1,
        "turn runtime context must use latest-wins replacement"
    );
    assert!(ctx.messages().iter().all(|message| {
        message
            .text_content()
            .is_none_or(|content| !content.contains("user prefers Rust over Python"))
    }));
    Ok(())
}

// ── save_transcript_projection ──────────────────────────────────────────────

/// Verify `AgentRunSnapshot::save_transcript_projection` calls
/// `ConversationStore::save_messages` with the projection of in-memory messages,
/// and silently no-ops when no `conversation_id` is configured.
#[tokio::test]
async fn save_transcript_projection_writes_to_conversation_store() {
    use crate::agent::snapshot::AgentRunSnapshot;
    use crate::memory::{
        Conversation, ConversationFilter, ConversationMeta, ConversationStore, NewConversation,
        StoredMessage,
    };
    use echo_core::error::Result as CoreResult;
    use futures::future::BoxFuture;
    use std::sync::Mutex as StdMutex;

    /// Minimal in-memory ConversationStore that records every save_messages call.
    struct RecordingStore {
        saves: StdMutex<Vec<(String, Vec<StoredMessage>)>>,
        conversations: StdMutex<Vec<Conversation>>,
    }

    impl RecordingStore {
        fn new() -> Self {
            Self {
                saves: StdMutex::new(Vec::new()),
                conversations: StdMutex::new(Vec::new()),
            }
        }
    }

    impl ConversationStore for RecordingStore {
        fn create_conversation<'a>(
            &'a self,
            conv: NewConversation,
        ) -> BoxFuture<'a, CoreResult<Conversation>> {
            Box::pin(async move {
                let c = Conversation {
                    id: 1,
                    conversation_id: conv.conversation_id,
                    user_id: conv.user_id,
                    agent_type: conv.agent_type,
                    title: conv.title,
                    summary: None,
                    compressed_before_id: None,
                    created_at: "now".to_string(),
                    updated_at: "now".to_string(),
                };
                self.conversations.lock().unwrap().push(c.clone());
                Ok(c)
            })
        }

        fn get_conversation<'a>(
            &'a self,
            conversation_id: &'a str,
        ) -> BoxFuture<'a, CoreResult<Option<Conversation>>> {
            Box::pin(async move {
                Ok(self
                    .conversations
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|c| c.conversation_id == conversation_id)
                    .cloned())
            })
        }

        fn list_conversations<'a>(
            &'a self,
            _filter: ConversationFilter,
        ) -> BoxFuture<'a, CoreResult<Vec<ConversationMeta>>> {
            Box::pin(async move { Ok(Vec::new()) })
        }

        fn update_conversation<'a>(
            &'a self,
            _conversation_id: &'a str,
            _title: Option<&'a str>,
            _summary: Option<&'a str>,
            _compressed_before_id: Option<i64>,
        ) -> BoxFuture<'a, CoreResult<()>> {
            Box::pin(async move { Ok(()) })
        }

        fn delete_conversation<'a>(
            &'a self,
            _conversation_id: &'a str,
        ) -> BoxFuture<'a, CoreResult<()>> {
            Box::pin(async move { Ok(()) })
        }

        fn save_messages<'a>(
            &'a self,
            conversation_id: &'a str,
            messages: &'a [StoredMessage],
        ) -> BoxFuture<'a, CoreResult<()>> {
            let key = conversation_id.to_string();
            let msgs = messages.to_vec();
            Box::pin(async move {
                self.saves.lock().unwrap().push((key, msgs));
                Ok(())
            })
        }

        fn get_messages<'a>(
            &'a self,
            _conversation_id: &'a str,
        ) -> BoxFuture<'a, CoreResult<Vec<StoredMessage>>> {
            Box::pin(async move { Ok(Vec::new()) })
        }

        fn count_messages<'a>(
            &'a self,
            _conversation_id: &'a str,
        ) -> BoxFuture<'a, CoreResult<usize>> {
            Box::pin(async move { Ok(0) })
        }
    }

    // ── Case 1: conversation_id + store both set → save_messages is invoked ──
    let store = Arc::new(RecordingStore::new());
    let config =
        AgentConfig::new("test-model", "agent", "sys").conversation_id("conv-projection-test");
    let mut agent = ReactAgent::new(config);
    agent.set_conversation_store(store.clone());
    // Seed two user messages.
    {
        let mut ctx = agent.memory.context.lock().await;
        ctx.push(Message::user("hello".to_string()));
        ctx.push(Message::user("world".to_string()));
    }

    let snap = AgentRunSnapshot::from_agent(&agent);
    snap.save_transcript_projection(&agent.memory.context).await;

    let saves = store.saves.lock().unwrap().clone();
    assert_eq!(saves.len(), 1, "save_messages should fire exactly once");
    let (saved_conv_id, saved_msgs) = &saves[0];
    assert_eq!(saved_conv_id, "conv-projection-test");
    assert!(
        saved_msgs.len() >= 2,
        "projection should include the two pushed user messages, got {} messages",
        saved_msgs.len()
    );
    let user_texts: Vec<&str> = saved_msgs
        .iter()
        .filter(|m| m.role == "user")
        .filter_map(|m| m.content.as_deref())
        .collect();
    assert!(user_texts.contains(&"hello"));
    assert!(user_texts.contains(&"world"));

    // ── Case 2: missing conversation_id → no-op ──
    let store2 = Arc::new(RecordingStore::new());
    let config2 = AgentConfig::new("test-model", "agent", "sys"); // no conversation_id
    let mut agent2 = ReactAgent::new(config2);
    agent2.set_conversation_store(store2.clone());
    let snap2 = AgentRunSnapshot::from_agent(&agent2);
    snap2
        .save_transcript_projection(&agent2.memory.context)
        .await;
    assert!(
        store2.saves.lock().unwrap().is_empty(),
        "without conversation_id, save_transcript_projection must early-return without saving"
    );
}
