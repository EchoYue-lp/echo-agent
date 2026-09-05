//! 子智能体通信原语:上行通道(uplink)与共享控制面
//!
//! 本示例不需要 LLM API Key,使用自定义 Agent 演示框架新增的通信原语:
//!
//! | 能力 | 演示方式 |
//! |------|---------|
//! | 身份/亲缘(lineage) | `dispatch_attempt` 派发时自动盖章(execution_id/task_id/attempt) |
//! | 共享控制面 | 一个 `SubagentRegistry` 一个控制面,execution_id 跨执行器可寻址 |
//! | 上行通道 | `default_uplink_sink`:Parent 方向 steer 父 / Sibling 方向 queue-only |
//! | 事件可观测 | raw bus 收到 `UplinkReceived`; envelope bus 提供执行顺序与恢复 |
//!
//! ## 运行方式
//!
//! ```bash
//! cargo run -p echo-agent-learning --example demo50_subagent_communication --features subagent
//! ```

use echo_agent::agent::{Agent, AgentEvent, AgentSteerError, AgentSteerReceipt, CancellationToken};
use echo_agent::error::Result;
use echo_agent::llm::types::Message;
use echo_agent::subagent::{
    DispatchRequest, SubagentAttemptIdentity, SubagentDefinition, SubagentEvent, SubagentExecutor,
    SubagentExecutorConfig, SubagentRegistry, default_uplink_sink,
};
use echo_agent::tools::{SubagentUplinkKind, SubagentUplinkMessage, SubagentUplinkTarget};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// 可转向的阻塞 Subagent:stream 首个 poll 等待 release,steer 输入被记录。
struct SteerableSubagent {
    release: Arc<Notify>,
    steered: Arc<Mutex<Vec<String>>>,
}

impl SteerableSubagent {
    fn blocking_stream<'a>(&'a self) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let release = self.release.clone();
        Box::pin(async move {
            Ok(Box::pin(futures::stream::once(async move {
                release.notified().await;
                Ok(AgentEvent::FinalAnswer("subagent finished".to_string()))
            })) as BoxStream<'a, Result<AgentEvent>>)
        })
    }

    fn new(release: Arc<Notify>, steered: Arc<Mutex<Vec<String>>>) -> Self {
        Self { release, steered }
    }
}

impl Agent for SteerableSubagent {
    fn name(&self) -> &str {
        "subagent"
    }

    fn model_name(&self) -> &str {
        "demo"
    }

    fn system_prompt(&self) -> &str {
        "demo subagent"
    }

    fn execute<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async { Ok("done".to_string()) })
    }

    fn execute_stream<'a>(
        &'a self,
        _task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.blocking_stream()
    }

    fn execute_stream_message_with_cancel<'a>(
        &'a self,
        _message: Message,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.blocking_stream()
    }

    fn steer_input_tracked(
        &self,
        _expected_turn_id: Option<&str>,
        message: Message,
    ) -> std::result::Result<AgentSteerReceipt, AgentSteerError> {
        let text = message
            .content
            .as_text()
            .filter(|text| !text.trim().is_empty())
            .ok_or(AgentSteerError::EmptyInput)?
            .to_string();
        println!("  [steer 注入] subagent 收到: {text}");
        let (_tx, rx) = tokio::sync::watch::channel(echo_agent::agent::AgentSteerState::Drained);
        self.steered
            .lock()
            .map_err(|_| AgentSteerError::StateUnavailable)?
            .push(text);
        Ok(AgentSteerReceipt::new(
            uuid::Uuid::new_v4().to_string(),
            "turn".to_string(),
            rx,
        ))
    }
}

#[tokio::main]
async fn main() {
    println!("=== demo50: 子智能体上行通信(uplink + 共享控制面) ===\n");

    let registry = Arc::new(SubagentRegistry::new());

    let release = Arc::new(Notify::new());
    let steered = Arc::new(Mutex::new(Vec::new()));
    registry
        .register(
            SubagentDefinition::new("subagent", "演示上行通道的工作子智能体"),
            Box::new(SteerableSubagent::new(release.clone(), steered.clone())),
        )
        .await;

    let mut events = registry.event_bus().subscribe();
    let mut execution_events = registry.event_bus().subscribe_envelopes();

    // 1) 以精确身份派发(subagent 阻塞,直到 release)
    let identity =
        match SubagentAttemptIdentity::new("task-b", "run-1:task-b:1:attempt:1:claim-x", 1) {
            Ok(identity) => identity,
            Err(error) => {
                eprintln!("identity 构造失败: {error}");
                return;
            }
        };
    let dispatch = tokio::spawn({
        let executor = SubagentExecutor::new(registry.clone(), SubagentExecutorConfig::default());
        async move {
            executor
                .dispatch_attempt(
                    DispatchRequest {
                        agent_name: "subagent".to_string(),
                        task: "被阻塞的任务".to_string(),
                        mode_override: None,
                        cancel: CancellationToken::new(),
                        parent_agent: "primary".to_string(),
                        parent_context: None,
                        delegation_policy: DispatchRequest::policy_from_depth(0),
                        runtime_context: None,
                        message: None,
                        prompt_payload: None,
                        prompt_context: None,
                        constraints: Vec::new(),
                        background: false,
                    },
                    identity,
                )
                .await
        }
    });

    // 等待 attempt 在共享控制面可见
    let execution_id = "run-1:task-b:1:attempt:1:claim-x".to_string();
    let mut became_active = false;
    for _ in 0..250 {
        if registry
            .control_registry()
            .active_snapshot(16)
            .iter()
            .any(|summary| summary.execution_id == execution_id)
        {
            became_active = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(became_active, "attempt 未在共享控制面出现");
    println!("1) subagent 已派发,execution_id = {execution_id}");

    // 2) 兄弟消息:经默认上行 sink queue-only 注入
    let sink = default_uplink_sink(Arc::clone(&registry));
    let sibling = sink(SubagentUplinkMessage {
        from: echo_agent::tools::SubagentLineage {
            agent_name: Some("subagent".to_string()),
            execution_id: Some(execution_id.clone()),
            parent_agent: Some("primary".to_string()),
            ..Default::default()
        },
        target: SubagentUplinkTarget::Sibling {
            to: echo_agent::tools::SubagentPeerAddress::ByExecutionId(execution_id.clone()),
            text: "依赖产物已就绪,请继续".to_string(),
        },
    })
    .await;
    println!(
        "2) 兄弟消息 → accepted={} status={}",
        sibling.accepted, sibling.status
    );

    // 3) 上行父方向:escalate(subagent 把发送者视为父,steer 父的活动 turn)
    let escalate = sink(SubagentUplinkMessage {
        from: echo_agent::tools::SubagentLineage {
            agent_name: Some("subagent".to_string()),
            execution_id: Some(execution_id.clone()),
            parent_agent: Some("primary".to_string()),
            parent_execution_id: Some(execution_id.clone()),
            ..Default::default()
        },
        target: SubagentUplinkTarget::Parent {
            kind: SubagentUplinkKind::Escalate,
            text: "计划假设有误,需要澄清目标约束".to_string(),
        },
    })
    .await;
    println!(
        "3) 父方向 escalate → accepted={} status={}",
        escalate.accepted, escalate.status
    );

    // 4) 事件可观测:事件总线上应有 UplinkReceived
    let mut uplink_events = 0;
    while let Ok(event) = events.try_recv() {
        if let SubagentEvent::UplinkReceived {
            direction, status, ..
        } = event.as_ref()
        {
            uplink_events += 1;
            println!("4) UplinkReceived: direction={direction} status={status}");
        }
    }
    assert!(uplink_events >= 2, "上行事件未全部可观测");

    // 5) 释放 subagent,结算派发
    release.notify_one();
    let result = match dispatch.await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            eprintln!("派发失败: {error}");
            return;
        }
        Err(error) => {
            eprintln!("join 失败: {error}");
            return;
        }
    };
    println!(
        "5) subagent 结算: status={:?}, steer 收到 {} 条消息",
        result.outcome.status,
        steered.lock().map(|m| m.len()).unwrap_or(0)
    );

    // 6) execution envelope 是身份、顺序和恢复的权威；raw bus 只用于兼容。
    let mut stream_id = None;
    let mut last_sequence = 0;
    while let Ok(envelope) = execution_events.try_recv() {
        println!(
            "6) envelope: sequence={} event_id={}",
            envelope.sequence, envelope.event_id
        );
        last_sequence = envelope.sequence;
        stream_id = Some(envelope.stream_id.clone());
    }
    let Some(stream_id) = stream_id else {
        eprintln!("execution envelope 未到达");
        return;
    };
    let retained_streams = registry.event_bus().retained_stream_ids();
    let active_streams = registry.event_bus().active_stream_ids();
    let replay = registry.event_bus().replay_after(&stream_id, 0);
    println!(
        "   retained_streams={} active_streams={} replay={} gap={:?} terminal={} last_sequence={last_sequence}",
        retained_streams.len(),
        active_streams.len(),
        replay.events.len(),
        replay.gap,
        replay.terminal.is_some()
    );

    println!("\n=== demo50 完成 ===");
}
