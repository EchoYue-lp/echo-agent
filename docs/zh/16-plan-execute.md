# Plan-and-Execute Agent —— 结构化任务执行

## 是什么

Plan-and-Execute 将**规划**和**执行**分离为两个独立阶段。规划器首先将任务分解为步骤，然后执行器逐个执行。

这是 ReAct 的替代执行策略，适合结构化、可预测的任务。

---

## 解决什么问题

ReAct 在每一步做临时决策。对于复杂多步任务，这可能导致：

- **缺乏全局视角**：没有明确的计划可遵循
- **低效重试**：步骤失败时，不清楚该重做哪些
- **无并行性**：无法利用独立的子任务

Plan-and-Execute 遵循"先规划，后执行"，显式分解任务。

---

## 架构

```
┌─────────────────────────────────────────────────────────────────────┐
│                  Plan-and-Execute 管道                               │
│                                                                      │
│   阶段 1: 规划                                                      │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  任务 → [Planner] → 计划 { 步骤1, 步骤2, ..., 步骤n }       │   │
│   └─────────────────────────────────────────────────────────────┘   │
│                              │                                       │
│                              ▼                                       │
│   阶段 2: 执行 (DAG)                                                │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │                                                              │   │
│   │     步骤1 ──┬──▶ 步骤2 ──┬──▶ 步骤4 ──▶ 汇总               │   │
│   │             │              │                                │   │
│   │             └──▶ 步骤3 ──┘                                │   │
│   │                                                              │   │
│   │  [TaskManager] 按拓扑序调度，独立步骤自动并行               │   │
│   └─────────────────────────────────────────────────────────────┘   │
│                                                                      │
│   阶段 3: 重规划（失败时）                                          │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  步骤2 失败 → 识别受影响的下游步骤 → 重新规划子图           │   │
│   └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 核心 Trait

### Planner

```rust
pub trait Planner: Send + Sync {
    fn plan<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<Plan>>;
}

pub struct Plan {
    pub steps: Vec<PlanStep>,
}

pub struct PlanStep {
    pub description: String,
    pub dependencies: Vec<String>,  // 前置步骤 ID
    pub status: StepStatus,
}
```

### Executor

```rust
pub trait Executor: Send + Sync {
    fn execute_step<'a>(
        &'a mut self,
        step_description: &'a str,
        context: &'a str,  // 前序步骤的结果
    ) -> BoxFuture<'a, Result<String>>;
}
```

---

## 规划器类型

### StaticPlanner

预定义步骤：

```rust
use echo_agent::agent::plan_execute::StaticPlanner;

let planner = StaticPlanner::new(vec![
    "搜索相关文档",
    "提取关键信息",
    "撰写摘要",
]);
```

### LlmPlanner

基于 LLM 的动态规划：

```rust
use echo_agent::agent::plan_execute::LlmPlanner;

let planner = LlmPlanner::new("qwen3-max")
    .with_output_mode(PlannerOutputMode::Structured);  // 或 NaturalLanguage
```

---

## 执行器类型

### SimpleExecutor

包装任意 Agent：

```rust
use echo_agent::agent::plan_execute::SimpleExecutor;

let agent = ReactAgentBuilder::simple("qwen3-max", "执行器")?;
let executor = SimpleExecutor::new(agent);
```

### ReactExecutor

带工具的 ReAct Agent：

```rust
use echo_agent::agent::plan_execute::ReactExecutor;

let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .name("executor")
    .system_prompt("执行给定的步骤")
    .enable_tools()
    .build()?;

let executor = ReactExecutor::new(agent);
```

---

## 使用示例

```rust
use echo_agent::prelude::*;
use echo_agent::agent::plan_execute::{PlanExecuteAgent, LlmPlanner, ReactExecutor};

#[tokio::main]
async fn main() -> Result<()> {
    let planner = LlmPlanner::new("qwen3-max");

    let executor_agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("executor")
        .system_prompt("你是任务执行者")
        .enable_tools()
        .build()?;
    let executor = ReactExecutor::new(executor_agent);

    let mut agent = PlanExecuteAgent::new("plan_agent", planner, executor)
        .max_replans(3);

    let result = agent
        .execute("研究 Rust 异步模式并撰写摘要报告")
        .await?;

    println!("{}", result);
    Ok(())
}
```

---

## 执行模式

### Sequential（默认）

步骤按依赖顺序逐一执行：

```rust
let agent = PlanExecuteAgent::new("planner", planner, executor)
    .with_execution_mode(ExecutionMode::Sequential);
```

### Parallel

独立步骤并发执行：

```rust
let execute_fn: TaskExecuteFn = Arc::new(|ctx| {
    Box::pin(async move { Ok(format!("完成: {}", ctx.task_id)) })
});

let agent = PlanExecuteAgent::new("planner", planner, executor)
    .with_execute_fn(execute_fn)
    .max_concurrent(5);
```

---

## 增量重规划

步骤失败时，仅重规划受影响的下游步骤：

```rust
// 步骤2 失败 → 识别下游: [步骤3, 步骤4]
// 从 TaskManager 移除受影响的任务
// 为受影响子图生成新计划
// 继续执行
```

这避免了重做已完成的工作。

---

## 计划验证

计划自动验证和修复：

```rust
let issues = plan.validate();
for issue in &issues {
    match issue.severity {
        IssueSeverity::Error => warn!("计划错误: {}", issue.message),
        IssueSeverity::Warning => info!("计划警告: {}", issue.message),
    }
}
plan.auto_fix();  // 修复常见问题
```

---

## 流式事件

```rust
let mut stream = agent.execute_stream(task).await?;

while let Some(event) = stream.next().await {
    match event? {
        AgentEvent::PlanGenerated { steps } => {
            println!("计划创建完成，共 {} 步骤", steps.len());
        }
        AgentEvent::StepStart { step_index, description } => {
            println!("步骤 {}: {}", step_index + 1, description);
        }
        AgentEvent::StepEnd { step_index, success } => {
            println!("步骤 {} {}", step_index + 1, if success { "✓" } else { "✗" });
        }
        AgentEvent::FinalAnswer(answer) => {
            println!("结果: {}", answer);
            break;
        }
        _ => {}
    }
}
```

---

## 适用场景

| 场景 | 适合程度 | 原因 |
|------|----------|------|
| 结构化多步任务 | ★★★★★ | 显式计划，可预测 |
| DAG 工作流 | ★★★★★ | 拓扑调度 |
| 需要失败恢复 | ★★★★☆ | 增量重规划 |
| 开放式探索 | ★★☆☆☆ | 难以预先规划 |

---

## vs ReAct

| 维度 | ReAct | Plan-and-Execute |
|------|-------|------------------|
| 规划 | 每步临时决策 | 前期显式规划 |
| 并行性 | 无 | 有（独立步骤） |
| 重规划 | 隐式重试 | 增量子图 |
| 可调试性 | 中 | 高（可见计划） |
| Token 成本 | 中 | 高（规划阶段） |

对应示例：`examples/demo22_plan_execute.rs`