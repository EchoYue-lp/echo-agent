use super::ReactAgent;
use crate::agent::Agent;
use crate::agent::config::AgentConfig;
use crate::llm::types::Message;
use crate::testing::{FailingMockAgent, MockAgent, MockTool};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ── ReactAgent::reset() ───────────────────────────────────────────────────────

/// reset() 应清除所有消息，仅保留 system prompt（1 条）
#[test]
fn react_agent_reset_clears_to_system_only() {
    let config = AgentConfig::new("test-model", "test_agent", "你是测试助手");
    let mut agent = ReactAgent::new(config);

    let (count, _) = agent.context_stats();
    assert_eq!(count, 1, "初始应只有 1 条 system 消息");

    agent.context.push(Message::user("你好".to_string()));
    agent.context.push(Message::assistant("你好！".to_string()));
    agent.context.push(Message::user("再见".to_string()));
    let (count_after_push, _) = agent.context_stats();
    assert_eq!(count_after_push, 4, "追加后应有 4 条消息");

    agent.reset();
    let (count_after_reset, _) = agent.context_stats();
    assert_eq!(count_after_reset, 1, "reset() 后应只剩 1 条 system 消息");
}

/// 连续 reset() 多次应幂等，不产生重复的 system prompt
#[test]
fn react_agent_reset_is_idempotent() {
    let config = AgentConfig::new("test-model", "test_agent", "系统提示词");
    let mut agent = ReactAgent::new(config);

    agent.reset();
    agent.reset();
    agent.reset();

    let (count, _) = agent.context_stats();
    assert_eq!(count, 1, "多次 reset() 后应仍只有 1 条 system 消息");
}

/// reset() 后 system prompt 内容应保持不变
#[test]
fn react_agent_reset_preserves_system_prompt() {
    let system = "这是一个自定义的系统提示词";
    let config = AgentConfig::new("test-model", "agent", system);
    let mut agent = ReactAgent::new(config);

    agent
        .context
        .push(Message::user("随便什么消息".to_string()));
    agent.reset();

    let messages = agent.context.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[0].content.as_deref().unwrap_or(""), system);
}

// ── ReactAgent 工具注册测试 ───────────────────────────────────────────────────────

/// 注意：ReactAgent::new 会自动注册 FinalAnswerTool
#[test]
fn react_agent_add_tool_enables_tool_flag() {
    let config = AgentConfig::minimal("test-model", "helper");
    assert!(!config.is_tool_enabled(), "minimal 配置默认不启用工具");

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(MockTool::new("test_tool")));

    assert!(agent.config().is_tool_enabled(), "add_tool 后应启用工具");
    // FinalAnswerTool + test_tool
    let tool_names = agent.tool_names();
    assert!(tool_names.iter().any(|n| *n == "test_tool"));
    assert!(tool_names.iter().any(|n| *n == "final_answer"));
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
    // FinalAnswerTool + 3 个自定义工具 = 4
    assert_eq!(tool_names.len(), 4);
    assert!(tool_names.iter().any(|n| *n == "tool1"));
    assert!(tool_names.iter().any(|n| *n == "tool2"));
    assert!(tool_names.iter().any(|n| *n == "tool3"));
}

#[test]
fn react_agent_add_tools_empty_vec() {
    let config = AgentConfig::minimal("test-model", "helper");
    let mut agent = ReactAgent::new(config);

    agent.add_tools(vec![]);

    assert!(
        !agent.config().is_tool_enabled(),
        "空工具列表不应修改 enable_tool"
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
    // FinalAnswerTool + allowed_tool = 2 (白名单只过滤用户添加的工具)
    assert_eq!(tool_names.len(), 2);
    assert!(tool_names.iter().any(|n| *n == "allowed_tool"));
}

// ── ReactAgent getter 方法测试 ───────────────────────────────────────────────────────

#[test]
fn react_agent_tool_names() {
    let config = AgentConfig::minimal("test-model", "helper");
    let mut agent = ReactAgent::new(config);

    // ReactAgent::new 会自动注册 FinalAnswerTool
    assert_eq!(agent.tool_names().len(), 1);

    agent.add_tool(Box::new(MockTool::new("tool1")));
    agent.add_tool(Box::new(MockTool::new("tool2")));

    let names = agent.tool_names();
    assert_eq!(names.len(), 3); // FinalAnswerTool + tool1 + tool2
}

#[test]
fn react_agent_skill_names() {
    let config = AgentConfig::minimal("test-model", "helper");
    let agent = ReactAgent::new(config);

    assert!(agent.skill_names().is_empty(), "初始应无技能");
}

#[test]
fn react_agent_mcp_server_names() {
    let config = AgentConfig::minimal("test-model", "helper");
    let agent = ReactAgent::new(config);

    assert!(agent.mcp_server_names().is_empty(), "初始应无 MCP 服务器");
}

#[test]
fn react_agent_get_messages() {
    let config = AgentConfig::new("test-model", "agent", "You are helpful");
    let mut agent = ReactAgent::new(config);

    let messages = agent.get_messages();
    assert_eq!(messages.len(), 1, "初始只有 system 消息");
    assert_eq!(messages[0].role, "system");

    agent.context.push(Message::user("Hello".to_string()));
    let messages = agent.get_messages();
    assert_eq!(messages.len(), 2);
}

#[test]
fn react_agent_context_stats() {
    let config = AgentConfig::new("test-model", "agent", "System prompt");
    let mut agent = ReactAgent::new(config);

    let (count, _tokens) = agent.context_stats();
    assert_eq!(count, 1);

    agent
        .context
        .push(Message::user("This is a test message".to_string()));
    let (count, tokens) = agent.context_stats();
    assert_eq!(count, 2);
    assert!(tokens > 0, "token 估算应大于 0");
}

// ── ReactAgent 配置测试 ───────────────────────────────────────────────────────

#[test]
fn react_agent_set_model() {
    let config = AgentConfig::minimal("model1", "helper");
    let mut agent = ReactAgent::new(config);

    assert_eq!(agent.model_name(), "model1");

    agent.set_model("model2");

    assert_eq!(agent.model_name(), "model2");
}

#[test]
fn react_agent_set_system_prompt() {
    let config = AgentConfig::minimal("test-model", "helper");
    let mut agent = ReactAgent::new(config);

    let _original_prompt = agent.system_prompt().to_string();
    agent.set_system_prompt("New system prompt".to_string());

    assert_eq!(agent.system_prompt(), "New system prompt");

    // 验证上下文中的 system 消息也已更新
    let messages = agent.get_messages();
    assert_eq!(messages[0].content.as_deref().unwrap(), "New system prompt");
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

// ── ReactAgent 回调测试 ───────────────────────────────────────────────────────

/// 简单的回调计数器
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

    // 验证回调已添加（通过检查内部状态）
    // 由于 callbacks 是私有的，我们只能通过执行来验证
    // 这里简单验证方法不会 panic
}

// ── Agent trait 合约 ──────────────────────────────────────────────────────────

/// reset() 可通过 &mut dyn Agent 调用（trait 对象安全性验证）
#[tokio::test]
async fn trait_reset_callable_via_dyn_agent() {
    let mut agent: Box<dyn Agent> = Box::new(
        MockAgent::new("mock")
            .with_response("r1")
            .with_response("r2"),
    );

    let r1 = agent.chat("msg1").await.unwrap();
    assert_eq!(r1, "r1");

    agent.reset();

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
    // MockAgent 默认 model_name 返回 "mock-model"
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

// ── MockAgent 合约 ────────────────────────────────────────────────────────────

/// chat() 应记录调用，并消费预设响应队列
#[tokio::test]
async fn mock_agent_chat_records_calls_and_consumes_responses() {
    let mut agent = MockAgent::new("test")
        .with_response("回复1")
        .with_response("回复2")
        .with_response("回复3");

    let r1 = agent.chat("消息1").await.unwrap();
    let r2 = agent.chat("消息2").await.unwrap();
    let r3 = agent.chat("消息3").await.unwrap();

    assert_eq!(r1, "回复1");
    assert_eq!(r2, "回复2");
    assert_eq!(r3, "回复3");
    assert_eq!(agent.call_count(), 3);
    assert_eq!(agent.calls(), vec!["消息1", "消息2", "消息3"]);
}

/// reset() 应清空 MockAgent 的调用历史（模拟对话重置语义）
#[tokio::test]
async fn mock_agent_reset_clears_call_history() {
    let mut agent = MockAgent::new("test")
        .with_response("r1")
        .with_response("r2")
        .with_response("r3");

    agent.chat("第一轮消息1").await.unwrap();
    agent.chat("第一轮消息2").await.unwrap();
    assert_eq!(agent.call_count(), 2, "reset 前应有 2 条记录");

    agent.reset();
    assert_eq!(agent.call_count(), 0, "reset 后调用历史应清空");

    agent.chat("第二轮消息1").await.unwrap();
    assert_eq!(agent.call_count(), 1, "reset 后第二轮应从 1 开始计数");
    assert_eq!(agent.calls(), vec!["第二轮消息1"]);
}

/// execute() 和 chat() 共享同一个响应队列
#[tokio::test]
async fn mock_agent_execute_and_chat_share_response_queue() {
    let mut agent = MockAgent::new("test")
        .with_response("execute回复")
        .with_response("chat回复");

    let r1 = agent.execute("任务").await.unwrap();
    let r2 = agent.chat("对话").await.unwrap();

    assert_eq!(r1, "execute回复");
    assert_eq!(r2, "chat回复");
    assert_eq!(agent.call_count(), 2);
}

/// 响应队列耗尽后，chat() 应返回默认响应
#[tokio::test]
async fn mock_agent_chat_falls_back_to_default_when_queue_empty() {
    let mut agent = MockAgent::new("test");

    let r = agent.chat("任意消息").await.unwrap();
    assert_eq!(r, "mock agent response", "队列空时应返回默认响应");
}

/// FailingMockAgent::reset() 清空调用历史
#[tokio::test]
async fn failing_mock_agent_reset_clears_calls() {
    let mut agent = FailingMockAgent::new("failing", "总是失败");

    agent.execute("任务1").await.unwrap_err();
    agent.chat("任务2").await.unwrap_err();
    assert_eq!(agent.call_count(), 2);

    agent.reset();
    assert_eq!(agent.call_count(), 0, "reset 后应清空调用记录");
}

// ── chat + reset 完整生命周期 ─────────────────────────────────────────────────

/// 模拟典型多轮对话生命周期：chat → reset → chat
#[tokio::test]
async fn mock_agent_full_chat_lifecycle() {
    let mut agent = MockAgent::new("assistant").with_responses([
        "轮1回复1",
        "轮1回复2",
        "轮2回复1",
        "轮2回复2",
    ]);

    agent.chat("第1轮：问题A").await.unwrap();
    agent.chat("第1轮：问题B").await.unwrap();
    assert_eq!(agent.call_count(), 2);

    agent.reset();
    assert_eq!(agent.call_count(), 0);

    agent.chat("第2轮：问题C").await.unwrap();
    agent.chat("第2轮：问题D").await.unwrap();
    assert_eq!(agent.call_count(), 2);
    assert_eq!(agent.calls(), vec!["第2轮：问题C", "第2轮：问题D"]);
}

// ── ReactAgentBuilder 测试 ───────────────────────────────────────────────────────

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
    // FinalAnswerTool + 内置工具（数量取决于启用的 feature）+ tool1 + tool2
    let names = agent.tool_names();
    assert!(names.iter().any(|n| *n == "tool1"), "应包含 tool1");
    assert!(names.iter().any(|n| *n == "tool2"), "应包含 tool2");
    assert!(
        names.iter().any(|n| *n == "final_answer"),
        "应包含 final_answer"
    );
    assert!(names.len() >= 3, "至少应有 3 个工具");
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

// ── ReactAgent 配置预设测试 ───────────────────────────────────────────────────────

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

// ── SubAgent 测试 ───────────────────────────────────────────────────────────────

#[test]
fn react_agent_register_subagent_requires_enable_flag() {
    // 不启用 subagent 功能
    let config = AgentConfig::minimal("test-model", "main_agent");
    let mut agent = ReactAgent::new(config);

    let sub_agent = Box::new(MockAgent::new("sub_agent"));
    agent.register_agent(sub_agent);

    // 由于 enable_subagent = false，subagent 不应被注册
    // 没有公开方法直接检查 subagent 列表，但可以通过行为验证
}

#[test]
fn react_agent_register_subagent_when_enabled() {
    let config = AgentConfig::minimal("test-model", "main_agent").enable_subagent(true);
    let mut agent = ReactAgent::new(config);

    let sub_agent = Box::new(MockAgent::new("sub_agent"));
    agent.register_agent(sub_agent);

    // subagent 应被成功注册
    // 可以通过检查 agent_dispatch 工具是否可用间接验证
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

    // 所有 subagent 应被成功注册
}

#[tokio::test]
async fn subagent_context_isolation() {
    // 创建父 agent
    let parent_config =
        AgentConfig::new("qwen3-max", "parent", "You are the parent agent").enable_subagent(true);
    let mut parent = ReactAgent::new(parent_config);

    // 父 agent 添加消息到上下文
    parent
        .context
        .push(Message::user("Parent message".to_string()));
    let (parent_count_before, _) = parent.context_stats();
    assert_eq!(parent_count_before, 2); // system + user message

    // 创建独立的子 agent
    let sub_config = AgentConfig::new("qwen3-max", "child", "You are a child agent");
    let mut child = ReactAgent::new(sub_config);

    // 子 agent 有自己的独立上下文
    let (child_count, _) = child.context_stats();
    assert_eq!(child_count, 1); // 只有 system 消息

    // 子 agent 添加消息不影响父 agent
    child
        .context
        .push(Message::user("Child message".to_string()));
    let (child_count_after, _) = child.context_stats();
    assert_eq!(child_count_after, 2);

    // 父 agent 的上下文不受影响
    let (parent_count_after, _) = parent.context_stats();
    assert_eq!(parent_count_after, 2);
}

#[tokio::test]
async fn subagent_reset_independence() {
    // 创建父 agent 和子 agent
    let parent_config =
        AgentConfig::new("qwen3-max", "parent", "Parent system").enable_subagent(true);
    let mut parent = ReactAgent::new(parent_config);

    let child_config = AgentConfig::new("qwen3-max", "child", "Child system");
    let mut child = ReactAgent::new(child_config);

    // 两者都添加消息
    parent.context.push(Message::user("Parent msg".to_string()));
    child.context.push(Message::user("Child msg".to_string()));

    // 重置父 agent
    parent.reset();

    // 父 agent 上下文被清空
    let (parent_count, _) = parent.context_stats();
    assert_eq!(parent_count, 1);

    // 子 agent 上下文不受影响
    let (child_count, _) = child.context_stats();
    assert_eq!(child_count, 2);
}

#[test]
fn react_agent_register_agent_dispatch_tool() {
    let config = AgentConfig::minimal("test-model", "main_agent").enable_subagent(true);
    let agent = ReactAgent::new(config);

    // 启用 subagent 后，agent_tool 工具应被注册
    let tool_names = agent.tool_names();
    assert!(tool_names.iter().any(|n| *n == "agent_tool"));
}

#[test]
fn react_agent_no_agent_dispatch_without_subagent() {
    let config = AgentConfig::minimal("test-model", "main_agent").enable_subagent(false);
    let agent = ReactAgent::new(config);

    // 不启用 subagent 时，agent_tool 工具不应被注册
    let tool_names = agent.tool_names();
    assert!(!tool_names.iter().any(|n| *n == "agent_tool"));
}

// ── Agent 配置隔离测试 ───────────────────────────────────────────────────────

#[test]
fn agent_config_isolation() {
    // 创建两个独立配置的 agent
    let config1 = AgentConfig::new("model-a", "agent1", "System A");
    let config2 = AgentConfig::new("model-b", "agent2", "System B");

    let agent1 = ReactAgent::new(config1);
    let agent2 = ReactAgent::new(config2);

    // 验证配置完全独立
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

    // agent1 注册工具
    agent1.add_tool(Box::new(MockTool::new("tool1")));

    // agent2 不应受影响
    let tools1 = agent1.tool_names();
    let tools2 = agent2.tool_names();

    // agent1 有 FinalAnswerTool + tool1
    assert_eq!(tools1.len(), 2);
    // agent2 只有 FinalAnswerTool
    assert_eq!(tools2.len(), 1);
}

#[test]
fn agent_callbacks_isolation() {
    let config1 = AgentConfig::minimal("model", "agent1");
    let config2 = AgentConfig::minimal("model", "agent2");

    let mut agent1 = ReactAgent::new(config1);
    let agent2 = ReactAgent::new(config2);

    // agent1 添加回调
    let callback = Arc::new(CounterCallback::new());
    agent1.add_callback(callback);

    // agent2 不应受影响（通过执行行为验证）
    // 由于 callbacks 是私有的，这里只验证方法不会 panic
    let _ = agent2;
}

// ── Agent Human-in-Loop 工具测试 ───────────────────────────────────────────────

#[test]
fn react_agent_human_in_loop_tool_registration() {
    let config = AgentConfig::minimal("model", "agent").enable_human_in_loop(true);
    let agent = ReactAgent::new(config);

    // 启用 human_in_loop 后，human_in_loop 工具应被注册
    let tool_names = agent.tool_names();
    assert!(tool_names.iter().any(|n| *n == "human_in_loop"));
}

#[test]
fn react_agent_no_human_in_loop_without_flag() {
    let config = AgentConfig::minimal("model", "agent").enable_human_in_loop(false);
    let agent = ReactAgent::new(config);

    // 不启用 human_in_loop 时，工具不应被注册
    let tool_names = agent.tool_names();
    assert!(!tool_names.iter().any(|n| *n == "human_in_loop"));
}

// ── Agent 任务规划工具测试 ───────────────────────────────────────────────────────

#[test]
fn react_agent_planning_tools_registration() {
    let config = AgentConfig::minimal("model", "agent").enable_task(true);
    let agent = ReactAgent::new(config);

    let tool_names = agent.tool_names();
    // 启用任务规划后应有相关工具
    assert!(tool_names.iter().any(|n| *n == "plan"));
    assert!(tool_names.iter().any(|n| *n == "create_task"));
    assert!(tool_names.iter().any(|n| *n == "update_task"));
    assert!(tool_names.iter().any(|n| *n == "list_tasks"));
}

#[test]
fn react_agent_no_planning_tools_without_flag() {
    let config = AgentConfig::minimal("model", "agent").enable_task(false);
    let agent = ReactAgent::new(config);

    let tool_names = agent.tool_names();
    // 不启用任务规划时不应有相关工具
    assert!(!tool_names.iter().any(|n| *n == "create_task"));
}

// ══════════════════════════════════════════════════════════════════════════════
// Feature 1: Memory Tool 自动注入（with_memory_tools + SearchMemoryTool）
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
    assert!(tools.iter().any(|n| *n == "remember"), "应注册 remember");
    assert!(tools.iter().any(|n| *n == "recall"), "应注册 recall");
    assert!(
        tools.iter().any(|n| *n == "search_memory"),
        "应注册 search_memory"
    );
    assert!(tools.iter().any(|n| *n == "forget"), "应注册 forget");
}

#[test]
fn builder_with_memory_tools_sets_store() {
    let store = Arc::new(crate::memory::store::InMemoryStore::new());
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("qwen3-max")
        .with_memory_tools(store)
        .build()
        .unwrap();

    assert!(agent.store().is_some(), "Store 应已设置");
}

#[test]
fn set_memory_store_registers_search_memory_tool() {
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);

    assert!(
        !agent.tool_names().iter().any(|n| *n == "search_memory"),
        "初始不应有 search_memory"
    );

    let store = Arc::new(crate::memory::store::InMemoryStore::new());
    agent.set_memory_store(store);

    assert!(
        agent.tool_names().iter().any(|n| *n == "search_memory"),
        "set_memory_store 后应有 search_memory"
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
        serde_json::Value::String("不存在的记忆".to_string()),
    );
    let result = tool.execute(params).await.unwrap();
    assert!(result.success);
    assert!(result.output.contains("未找到"));
}

// ══════════════════════════════════════════════════════════════════════════════
// Feature 2: Token 预算管控（max_tool_output_tokens）
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

#[test]
fn truncate_tool_output_no_limit() {
    let config = AgentConfig::new("model", "agent", "prompt");
    let agent = ReactAgent::new(config);
    let long_text = "a".repeat(10000);
    let result = agent.truncate_tool_output(long_text.clone());
    assert_eq!(result.len(), long_text.len(), "无限制时不应截断");
}

#[test]
fn truncate_tool_output_within_limit() {
    let config = AgentConfig::new("model", "agent", "prompt").max_tool_output_tokens(100000);
    let agent = ReactAgent::new(config);
    let short_text = "hello world".to_string();
    let result = agent.truncate_tool_output(short_text.clone());
    assert_eq!(result, short_text, "未超限时不应截断");
}

#[test]
fn truncate_tool_output_exceeds_limit() {
    let config = AgentConfig::new("model", "agent", "prompt").max_tool_output_tokens(10);
    let agent = ReactAgent::new(config);
    let long_text = "a ".repeat(500);
    let result = agent.truncate_tool_output(long_text);
    assert!(result.contains("[输出已截断"), "超限时应有截断提示");
    assert!(result.len() < 1000, "截断后应显著变短");
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
// Feature 5: 动态 Tool 注册/注销
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn remove_tool_basic() {
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(MockTool::new("tool_a")));
    agent.add_tool(Box::new(MockTool::new("tool_b")));

    assert!(agent.tool_names().iter().any(|n| *n == "tool_a"));

    let removed = agent.remove_tool("tool_a");
    assert!(removed.is_some(), "应返回被移除的工具");
    assert!(
        !agent.tool_names().iter().any(|n| *n == "tool_a"),
        "移除后不应存在"
    );
    assert!(
        agent.tool_names().iter().any(|n| *n == "tool_b"),
        "其他工具应不受影响"
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
    assert!(old.is_some(), "应返回旧工具");
    assert!(
        agent.tool_names().iter().any(|n| *n == "tool_x"),
        "新工具应存在"
    );
}

#[test]
fn replace_tool_when_not_exists() {
    let config = AgentConfig::minimal("model", "agent");
    let mut agent = ReactAgent::new(config);

    let old = agent.replace_tool(Box::new(MockTool::new("new_tool")));
    assert!(old.is_none(), "不存在旧工具时应返回 None");
    assert!(
        agent.tool_names().iter().any(|n| *n == "new_tool"),
        "新工具应已注册"
    );
}
