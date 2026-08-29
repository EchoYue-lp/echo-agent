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
| `DagWorkflow` | 拓扑调度，独立节点自动并行 | DAG 任务 |

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

```rust
use echo_agent::workflow::{Graph, MemoryCheckpointStore, CheckpointStore};

let checkpoint_store = MemoryCheckpointStore::new();

// 带检查点运行
let graph = GraphBuilder::new("long_running")
    // ... 节点定义
    .build()?;

let result = graph.run_with_checkpoints(state.clone(), &checkpoint_store).await?;

// 中断后从检查点恢复
let checkpoint_id = checkpoint_store.latest()?.id;
let resumed = graph.resume_from_checkpoint(&checkpoint_store, &checkpoint_id).await?;
```

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