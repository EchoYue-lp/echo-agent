# Graph Workflow —— LangGraph 风格 Agent 编排

## 是什么

Graph Workflow 将 Agent 执行建模为**有向图 + 共享状态**，支持：
- 线性管道
- 条件分支
- 循环
- 并行 fan-out/fan-in

这是 echo-agent 对 LangGraph 模式的实现。

---

## 核心概念

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Graph Workflow 结构                               │
│                                                                      │
│   SharedState: 节点间共享的 KV store + 结构化消息                   │
│                                                                      │
│   ┌─────────┐     ┌─────────┐     ┌─────────┐                      │
│   │ Node A  │────▶│ Node B  │────▶│ Node C  │                      │
│   │ (Agent) │     │(Function)│    │ (Agent) │                      │
│   └─────────┘     └─────────┘     └─────────┘                      │
│        │                                                │            │
│        │           ┌─────────┐                         │            │
│        └──────────▶│ Node D  │◀────────────────────────┘            │
│                    │(Condition)│    (条件边)                         │
│                    └─────────┘                                      │
│                         │                                           │
│                    ┌────┴────┐                                     │
│                    ▼         ▼                                     │
│               ┌────────┐ ┌────────┐                                │
│               │Node E  │ │Node F  │  (并行 fan-out)               │
│               └────────┘ └────────┘                                │
│                    │         │                                     │
│                    └────┬────┘                                     │
│                         ▼                                           │
│                    ┌────────┐                                       │
│                    │  END   │  (fan-in)                            │
│                    └────────┘                                       │
└─────────────────────────────────────────────────────────────────────┘
```

---

## SharedState

所有节点共享的状态：

```rust
use echo_agent::workflow::SharedState;

let state = SharedState::new();
state.set("input", "你好");
state.set("count", 42);

let input: String = state.get("input").unwrap_or_default();
let count: i32 = state.get("count").unwrap_or(0);

// 访问消息
for msg in state.messages() {
    println!("[{}] {}", msg.role, msg.content);
}
```

---

## GraphBuilder DSL

```rust
use echo_agent::workflow::{GraphBuilder, SharedState, shared_agent};

// 创建 Agent
let researcher = shared_agent(
    ReactAgentBuilder::simple("qwen3-max", "研究员")?
);
let writer = shared_agent(
    ReactAgentBuilder::simple("qwen3-max", "作者")?
);

let graph = GraphBuilder::new("research_pipeline")
    // Agent 节点：输入/输出键映射
    .add_agent_node("researcher", researcher.clone())
        .input_key("task")
        .output_key("research")
    .add_agent_node("writer", writer.clone())
        .input_key("research")
        .output_key("result")
    // 函数节点：数据转换
    .add_function_node("format", |state| Box::pin(async move {
        let result: String = state.get("result").unwrap_or_default();
        state.set("final", format!("### 报告\n\n{}", result));
        Ok(())
    }))
    // 图结构定义
    .set_entry("researcher")
    .add_edge("researcher", "writer")
    .add_edge("writer", "format")
    .set_finish("format")
    .build()?;

// 执行
let state = SharedState::new();
state.set("task", "研究 Rust 异步模式");
let result = graph.run(state).await?;
println!("{}", result.state.get::<String>("final").unwrap_or_default());
```

---

## 条件边

```rust
let graph = GraphBuilder::new("conditional_flow")
    .add_agent_node("analyzer", analyzer)
    .add_agent_node("detailed", detailed_agent)
    .add_agent_node("summary", summary_agent)
    // 基于状态的条件边
    .add_conditional_edge("analyzer", |state| {
        let complexity: f64 = state.get("complexity").unwrap_or(0.5);
        if complexity > 0.7 { "detailed" } else { "summary" }
    })
    .add_edge("detailed", "__end__")
    .add_edge("summary", "__end__")
    .set_entry("analyzer")
    .set_finish("__end__")
    .build()?;
```

---

## 并行 Fan-out/Fan-in

```rust
let graph = GraphBuilder::new("parallel_processing")
    .add_function_node("input", |state| Box::pin(async move {
        state.set("data", vec!["a", "b", "c"]);
        Ok(())
    }))
    .add_function_node("process_a", |state| Box::pin(async move {
        let data: Vec<String> = state.get("data").unwrap_or_default();
        state.set("result_a", format!("A: {:?}", data));
        Ok(())
    }))
    .add_function_node("process_b", |state| Box::pin(async move {
        let data: Vec<String> = state.get("data").unwrap_or_default();
        state.set("result_b", format!("B: {:?}", data));
        Ok(())
    }))
    .add_function_node("merge", |state| Box::pin(async move {
        let a: String = state.get("result_a").unwrap_or_default();
        let b: String = state.get("result_b").unwrap_or_default();
        state.set("final", format!("{}\n{}", a, b));
        Ok(())
    }))
    // Fan-out: input → process_a, process_b（并行）
    .set_entry("input")
    .add_edge("input", "process_a")
    .add_edge("input", "process_b")
    // Fan-in: process_a, process_b → merge
    .add_edge("process_a", "merge")
    .add_edge("process_b", "merge")
    .set_finish("merge")
    .build()?;
```

---

## 流式事件

```rust
let mut stream = graph.run_stream(state).await?;

while let Some(event) = stream.next().await {
    match event? {
        WorkflowEvent::NodeStart { node_name, step_index } => {
            println!("▶ 开始节点: {} (步骤 {})", node_name, step_index);
        }
        WorkflowEvent::NodeEnd { node_name, step_index, elapsed } => {
            println!("✓ 完成: {} 耗时 {:?}", node_name, elapsed);
        }
        WorkflowEvent::Token { node_name, token } => {
            print!("{}", token);  // 流式输出 agent 节点的 token
        }
        WorkflowEvent::NodeError { node_name, error } => {
            eprintln!("✗ 错误 {}: {}", node_name, error);
        }
        WorkflowEvent::Completed { result, total_steps, elapsed } => {
            println!("\n=== 工作流完成，耗时 {:?} ===", elapsed);
            println!("结果: {}", result);
        }
    }
}
```

`NodeError` 是失败终态的诊断事件：流会先发出该事件，随后以同一个错误结束，
不会再发出 `Completed`。并行分支遇到首个失败时会取消并排空其余 sibling，避免
挂起节点遮蔽错误或在调用方取消后继续产生副作用。
Agent 节点会把 `AgentEvent::Token` 转发为 `WorkflowEvent::Token`，并在发出
`NodeEnd` 前把最终答案提交到 `SharedState`。

`run`、`run_until_interrupt`、checkpoint resume 和 `run_stream` 的节点路由、
path/step 计数、fan-out、错误和完成结算全部委托给同一条内部执行循环。流式入口只是
该循环的事件投影，不是第二个执行器。

---

## 声明式 YAML 工作流

无需写 Rust 代码即可定义工作流：

```yaml
# workflow.yaml
name: content_pipeline
nodes:
  - name: researcher
    type: agent
    model: qwen3-max
    system_prompt: "你是一个研究助手"
    input_key: task
    output_key: research
    
  - name: writer
    type: agent
    model: qwen3-max
    system_prompt: "你是一个作者"
    input_key: research
    output_key: draft
    
  - name: reviewer
    type: agent
    model: qwen3-max
    system_prompt: "你是一个编辑"
    input_key: draft
    output_key: final

edges:
  - from: researcher
    to: writer
  - from: writer
    to: reviewer

entry: researcher
finish: [reviewer]
```

```rust
use echo_agent::workflow::Graph;

let graph = Graph::from_yaml("workflow.yaml")?;
let state = SharedState::new();
state.set("task", "撰写关于 Rust 所有权的文章");
let result = graph.run(state).await?;
```

---

## 工作流类型

| 类型 | 描述 | 用例 |
|------|------|------|
| `Graph` | LangGraph 风格，支持条件边、循环 | 复杂多 Agent 编排 |
| `SequentialWorkflow` | 简单管道，步骤 N 输出 → 步骤 N+1 输入 | ETL 管道 |
| `ConcurrentWorkflow` | 所有 Agent 并行执行，结果合并 | 并行分析 |
| `DagWorkflow` | 固定的无环 Agent 管道，前驱文本输出传给后继节点 | 静态管道调用 |

这些是公开的 Workflow API，与版本化 Task graph 分属不同权威。`Graph` 拥有条件
路由、共享状态和 checkpoint continuation；`DagWorkflow` 拥有一次固定拓扑的管道
调用，没有 Task claim 或 checkpoint 合同。动态 Task DAG 应使用
`RuntimeTaskService`。Task 派发 Workflow 时，其输出是精确 claim 结算的证据，
不是另一套 Task 状态权威。见 [ADR 0059](../adr/0059-task-workflow-dag-authority.md)。

### SequentialWorkflow

```rust
use echo_agent::workflow::{SequentialWorkflow, SequentialWorkflowBuilder, WorkflowStep};

let workflow = SequentialWorkflowBuilder::new()
    .add_step(WorkflowStep::agent("step1", agent1))
    .add_step(WorkflowStep::agent("step2", agent2))
    .add_step(WorkflowStep::function("format", |input| Box::pin(async move {
        Ok(format!("结果: {}", input))
    })))
    .build()?;

let result = workflow.run("初始输入").await?;
```

### ConcurrentWorkflow

```rust
use echo_agent::workflow::{ConcurrentWorkflow, ConcurrentWorkflowBuilder};

let workflow = ConcurrentWorkflowBuilder::new()
    .add_agent("analyzer1", agent1)
    .add_agent("analyzer2", agent2)
    .add_agent("analyzer3", agent3)
    .build()?;

let result = workflow.run("分析这些数据").await?;
// 所有 Agent 并行执行，结果合并
```

---

## 检查点与恢复

Graph 绑定自己的 `Arc<dyn CheckpointStore>`：用 `with_checkpoint_store` 选择
`MemoryCheckpointStore` 或 `FileCheckpointStore`。通过 `run_until_interrupt(state)`
运行到已配置的 before/after 中断，从 `RunUntilInterruptResult::Interrupted` 取出
`InterruptState.checkpoint`，再调用 `resume(checkpoint, ApprovalDecision::Approved)`。
需要修改恢复状态时使用 `resume_with_state`；分支使用 `branch_from`。基础 Graph/state
用法见已纳入可执行合同的 [demo39](https://github.com/EchoYue-lp/echo-agent/blob/main/echo-agent-learning/tests/example_contracts/demo39_workflow.rs)；
检查点入口以当前 [Graph 源码](https://github.com/EchoYue-lp/echo-agent/blob/main/echo-orchestration/src/workflow/graph.rs)为准。

可执行的 Approved/Modified 恢复要求该 Graph 绑定的 Store 声明
`supports_protected_claims()`，并在赢取原 claim 后、Node 副作用之前成功执行
`protect_claim(checkpoint_id, attempt_id)`。默认能力为 false；不支持时在 claim 之前拒绝，
`branch_from` 在创建分支检查点之前拒绝。Deferred 仍只加载/校验；Rejected 使用原
claim/ack 结算而不执行 Node。fresh `run`、`run_until_interrupt`、`run_stream`
不因此要求保护能力。自定义和 SDK-backed Store 须在同一 claim 权威上实现原子的保护，
以及原 renew、ack、requeue 和 generation-CAS；能力声明本身不是持久化证明。

只有原 continuation 真正结束后，成功结果才 ack，已知物理结束的普通错误才按原策略
requeue。保护失败或其确认丢失时不执行 Node，也不通过 ack/requeue 回滚未知保护。
`ExecutionNotSettled` 是观察知识，不是 Failed 或重试许可；无后续权威来源时保原 claim。
同一绑定 source 的后到真实 terminal 加实际 EOF 可以确认自己的观察，单独 EOF、
断连或本地 owner 消失不能。不同 Store 侧的观察不会被 Node 的本地 terminal 清除。

Graph 仍为原 attempt heartbeat 续租，但受保护的 claim 即使停止续租也不会因 TTL
自动回收或重投；真正未保护的旧记录保持原 lease 恢复策略。取消、节点超时、heartbeat
失败、fanout sibling 失败及 stream 关闭会发取消信号，并继续排空同一已开始的 source。
取消 borrowed waiter 或丢弃已 poll 的 stream 不会重新执行 Node。调用者应在未知观察后
保留原 Graph/Agent/Store parent；销毁所有 parent 不构成物理停止证明，也没有自动恢复承诺。

Memory 的保护只限同一 Store 实例；File 用原目录 OS 锁和版本化私有记录跨重启保留保护。
受保护记录不能被 expiry、save/delete/clear 隐式解除，混合 clear 会在任何删除前整体拒绝。
已验证的旧格式自动读取路径会拒绝新保护格式，但旧 delete/clear 不安全，禁止旧写者接管保护数据。
没有新增强制解锁或 receipt-query API。完整约束见
[ADR 0088](../adr/0088-subagent-settlement-observation.md) 与
[Rust 迁移说明](../subagent-cancellation-observation.md)。

批准 `BeforeNode` checkpoint 只会跳过它所代表的那一次中断，后续节点自己的中断仍会
正常检查。finish 节点与其它可执行节点使用相同的 before/after 中断语义；从 finish 后的
checkpoint 恢复时会沿保留路径完成，不会重复执行 finish 节点。

---

## 最佳实践

1. **保持节点职责单一**：每个节点做好一件事
2. **一致性使用 SharedState 键**：文档化输入/输出键
3. **函数节点中处理错误**：返回 `Err()` 停止工作流
4. **长时工作流使用流式**：提供实时反馈
5. **设计可恢复性**：关键工作流使用检查点

---

## 适用场景

| 场景 | 适合程度 | 原因 |
|------|----------|------|
| 多 Agent 管道 | ★★★★★ | 天然契合 |
| 条件分支流程 | ★★★★★ | 条件边支持 |
| 并行处理 | ★★★★★ | Fan-out/fan-in |
| 简单单 Agent | ★☆☆☆☆ | 过度设计 |

对应示例：`echo-agent-learning/tests/example_contracts/demo39_workflow.rs`、`echo-agent-learning/tests/example_contracts/demo34_workflow_stream.rs`、`echo-agent-learning/tests/example_contracts/demo37_declarative_workflow.rs`
