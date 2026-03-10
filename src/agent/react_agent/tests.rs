use super::ReactAgent;
use crate::agent::Agent;
use crate::agent::config::AgentConfig;
use crate::llm::types::Message;
use crate::testing::{FailingMockAgent, MockAgent};

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
