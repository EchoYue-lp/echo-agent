use super::ReactAgent;
use crate::agent::Agent;
use crate::agent::config::AgentConfig;
use crate::llm::types::{Message, Role};
use crate::sandbox::SandboxManager;
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
    assert!(tool_names.contains(&"test_tool"));
    assert!(tool_names.contains(&"final_answer"));
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
    assert!(tool_names.contains(&"tool1"));
    assert!(tool_names.contains(&"tool2"));
    assert!(tool_names.contains(&"tool3"));
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
    assert!(tool_names.contains(&"allowed_tool"));
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
    let mut agent = ReactAgent::new(config);

    let _original_prompt = agent.system_prompt().to_string();
    agent
        .set_system_prompt("New system prompt".to_string())
        .await;

    assert_eq!(agent.system_prompt(), "New system prompt");

    // Verify that the system message in context has also been updated
    let messages = agent.get_messages().await;
    assert_eq!(
        messages[0].content.as_text_ref().unwrap(),
        "New system prompt"
    );
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

/// When the response queue is exhausted, chat() should return a default response
#[tokio::test]
async fn mock_agent_chat_falls_back_to_default_when_queue_empty() {
    let agent = MockAgent::new("test");

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
    assert!(names.contains(&"tool1"), "Should contain tool1");
    assert!(names.contains(&"tool2"), "Should contain tool2");
    assert!(
        names.contains(&"final_answer"),
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
        .enable_planning()
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
        .token_limit(8000)
        .build()
        .unwrap();

    assert_eq!(agent.config().get_token_limit(), 8000);
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

#[test]
fn react_agent_register_subagent_when_enabled() {
    let config = AgentConfig::minimal("test-model", "main_agent").enable_subagent(true);
    let mut agent = ReactAgent::new(config);

    let sub_agent = Box::new(MockAgent::new("sub_agent"));
    agent.register_agent(sub_agent);

    // subagent should be successfully registered
    // Can indirectly verify by checking if agent_dispatch tool is available
}

#[test]
fn react_agent_register_multiple_subagents() {
    let config = AgentConfig::minimal("test-model", "main_agent").enable_subagent(true);
    let mut agent = ReactAgent::new(config);

    let sub_agents: Vec<Box<dyn Agent>> = vec![
        Box::new(MockAgent::new("worker1")),
        Box::new(MockAgent::new("worker2")),
        Box::new(MockAgent::new("worker3")),
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

#[test]
fn react_agent_register_agent_dispatch_tool() {
    let config = AgentConfig::minimal("test-model", "main_agent").enable_subagent(true);
    let agent = ReactAgent::new(config);

    // When subagent is enabled, agent_tool should be registered
    let tool_names = agent.tool_names();
    assert!(tool_names.contains(&"agent_tool"));
}

#[test]
fn react_agent_no_agent_dispatch_without_subagent() {
    let config = AgentConfig::minimal("test-model", "main_agent").enable_subagent(false);
    let agent = ReactAgent::new(config);

    // When subagent is not enabled, agent_tool should not be registered
    let tool_names = agent.tool_names();
    assert!(!tool_names.contains(&"agent_tool"));
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
        tools2.len() >= 1,
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

#[test]
fn react_agent_human_in_loop_tool_registration() {
    let config = AgentConfig::minimal("model", "agent").enable_human_in_loop(true);
    let agent = ReactAgent::new(config);

    // After enabling human_in_loop, the human_in_loop tool should be registered
    let tool_names = agent.tool_names();
    assert!(tool_names.contains(&"human_in_loop"));
}

#[test]
fn react_agent_no_human_in_loop_without_flag() {
    let config = AgentConfig::minimal("model", "agent").enable_human_in_loop(false);
    let agent = ReactAgent::new(config);

    // When human_in_loop is not enabled, the tool should not be registered
    let tool_names = agent.tool_names();
    assert!(!tool_names.contains(&"human_in_loop"));
}

#[tokio::test]
async fn add_need_appeal_tool_does_not_nest_runtime_with_permission_service() {
    let provider = Arc::new(crate::human_loop::HumanLoopManager::new());
    let service = Arc::new(crate::human_loop::PermissionService::from_provider(
        provider.clone() as Arc<dyn crate::human_loop::HumanLoopProvider>,
    ));

    let config = AgentConfig::minimal("model", "agent").enable_human_in_loop(true);
    let mut agent = ReactAgent::new(config);
    agent.set_permission_service(service.clone());

    // Before fix, this would trigger Handle::current().block_on(...) panic in async context.
    agent.add_need_appeal_tool(Box::new(MockTool::new("dangerous_tool")));

    agent.flush_pending_permission_rules(service.as_ref()).await;

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
    assert!(first_params.contains("skill-one"));
    assert!(!first_params.contains("skill-two"));

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
    assert!(first_activation.output.contains("Use skill one."));

    agent
        .discover_skills(&[DiscoveryScope::Custom(base.join("skills-b"))])
        .await
        .unwrap();

    let second_params = agent
        .tools
        .tool_manager
        .get_tool("activate_skill")
        .expect("activate_skill should stay registered")
        .parameters()
        .to_string();
    assert!(second_params.contains("skill-one"));
    assert!(second_params.contains("skill-two"));

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
    assert!(repeat_activation.output.contains("already activated"));

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
    assert!(second_activation.output.contains("Use skill two."));

    let _ = tokio::fs::remove_dir_all(base).await;
}

#[tokio::test]
async fn execute_tool_injects_pre_and_post_hook_messages_into_context() {
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

    let result = agent.execute_tool("test_tool", &json!({})).await.unwrap();
    assert_eq!(result, "tool ok");

    let messages: Vec<String> = agent
        .get_messages()
        .await
        .iter()
        .filter_map(|m| m.content.as_text_ref().map(str::to_string))
        .collect();
    assert!(messages.iter().any(|m| m.contains("pre-hook guidance")));
    assert!(messages.iter().any(|m| m.contains("post-hook guidance")));
}

#[tokio::test]
async fn shell_skill_uses_agent_sandbox_manager_when_present() {
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);
    agent.set_sandbox_manager(Arc::new(SandboxManager::local_only()));
    agent.add_skill(Box::new(ShellSkill::new()));

    let result = agent
        .execute_tool("shell", &json!({"command": "echo sandboxed"}))
        .await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("sandboxed"));
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
    assert!(missing.error.unwrap_or_default().contains("context_path"));

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
            .contains("cannot be activated")
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
    assert!(matched.output.contains("Lint the current Python file."));

    let _ = tokio::fs::remove_dir_all(base).await;
}

// ── Agent Task Planning Tool Tests ───────────────────────────────────────────────────────

#[test]
fn react_agent_planning_tools_registration() {
    let config = AgentConfig::minimal("model", "agent").enable_task(true);
    let agent = ReactAgent::new(config);

    let tool_names = agent.tool_names();
    // After enabling task planning, related tools should be available
    assert!(tool_names.contains(&"plan"));
    assert!(tool_names.contains(&"create_task"));
    assert!(tool_names.contains(&"update_task"));
    assert!(tool_names.contains(&"list_tasks"));
}

#[test]
fn react_agent_no_planning_tools_without_flag() {
    let config = AgentConfig::minimal("model", "agent").enable_task(false);
    let agent = ReactAgent::new(config);

    let tool_names = agent.tool_names();
    // When task planning is not enabled, related tools should not be available
    assert!(!tool_names.contains(&"create_task"));
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
    assert!(tools.contains(&"remember"), "Should register remember");
    assert!(tools.contains(&"recall"), "Should register recall");
    assert!(
        tools.contains(&"search_memory"),
        "Should register search_memory"
    );
    assert!(tools.contains(&"forget"), "Should register forget");
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
        !agent.tool_names().contains(&"search_memory"),
        "Should not have search_memory initially"
    );

    let store = Arc::new(crate::memory::store::InMemoryStore::new());
    agent.set_memory_store(store);

    assert!(
        agent.tool_names().contains(&"search_memory"),
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
    assert!(result.output.contains("No memories found"));
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
    let result = agent.truncate_tool_output(long_text.clone()).await;
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
    let result = agent.truncate_tool_output(short_text.clone()).await;
    assert_eq!(result, short_text, "Should not truncate when within limit");
}

#[tokio::test]
async fn truncate_tool_output_exceeds_limit() {
    let config = AgentConfig::new("model", "agent", "prompt").max_tool_output_tokens(10);
    let agent = ReactAgent::new(config);
    let long_text = "a ".repeat(500);
    let result = agent.truncate_tool_output(long_text).await;
    assert!(
        result.contains("[Output truncated"),
        "Should show truncation notice when over limit"
    );
    assert!(
        result.len() < 1000,
        "Should be significantly shorter after truncation"
    );
}

#[test]
fn config_compress_threshold_ratio_default() {
    let config = AgentConfig::new("model", "agent", "prompt");
    assert!((config.get_compress_threshold_ratio() - 0.2).abs() < f64::EPSILON);
}

#[test]
fn config_compress_threshold_ratio_custom() {
    let config = AgentConfig::new("model", "agent", "prompt").compress_threshold_ratio(0.5);
    assert!((config.get_compress_threshold_ratio() - 0.5).abs() < f64::EPSILON);
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

    assert!(agent.tool_names().contains(&"tool_a"));

    let removed = agent.remove_tool("tool_a");
    assert!(removed.is_some(), "Should return the removed tool");
    assert!(
        !agent.tool_names().contains(&"tool_a"),
        "Should not exist after removal"
    );
    assert!(
        agent.tool_names().contains(&"tool_b"),
        "Other tools should be unaffected"
    );
}

#[test]
fn remove_tool_nonexistent() {
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);
    let removed = agent.remove_tool("nonexistent");
    assert!(removed.is_none());
}

#[test]
fn replace_tool_basic() {
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(MockTool::new("tool_x")));

    let old = agent.replace_tool(Box::new(MockTool::new("tool_x")));
    assert!(old.is_some(), "Should return the old tool");
    assert!(
        agent.tool_names().contains(&"tool_x"),
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
        agent.tool_names().contains(&"new_tool"),
        "New tool should be registered"
    );
}
