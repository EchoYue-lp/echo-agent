# Plan-and-Execute Agent — Structured Task Execution

## What It Is

Plan-and-Execute separates **planning** and **execution** into two distinct phases. The Planner first decomposes a task into steps, then the Executor runs each step.

This is an alternative execution strategy to ReAct, suited for structured, predictable tasks.

---

## Problem It Solves

ReAct makes ad-hoc decisions at each step. For complex multi-step tasks, this can lead to:

- **Lack of global view**: No explicit plan to follow
- **Inefficient retry**: When a step fails, unclear what to redo
- **No parallelism**: Cannot exploit independent subtasks

Plan-and-Execute follows "plan first, then act" with explicit task decomposition.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                  Plan-and-Execute Pipeline                           │
│                                                                      │
│   Phase 1: Planning                                                 │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  Task → [Planner] → Plan { step_1, step_2, ..., step_n }   │   │
│   └─────────────────────────────────────────────────────────────┘   │
│                              │                                       │
│                              ▼                                       │
│   Phase 2: Execution (DAG)                                          │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │                                                              │   │
│   │     step_1 ──┬──▶ step_2 ──┬──▶ step_4 ──▶ Summary        │   │
│   │              │              │                               │   │
│   │              └──▶ step_3 ──┘                               │   │
│   │                                                              │   │
│   │  [TaskManager] schedules by topology, parallel for independent │
│   └─────────────────────────────────────────────────────────────┘   │
│                                                                      │
│   Phase 3: Replanning (on failure)                                  │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  step_2 fails → identify affected downstream → replan subgraph │
│   └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Core Traits

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
    pub dependencies: Vec<String>,  // IDs of prerequisite steps
    pub status: StepStatus,
}
```

### Executor

```rust
pub trait Executor: Send + Sync {
    fn execute_step<'a>(
        &'a mut self,
        step_description: &'a str,
        context: &'a str,  // Results from previous steps
    ) -> BoxFuture<'a, Result<String>>;
}
```

---

## Planner Types

### StaticPlanner

Pre-defined steps:

```rust
use echo_agent::agent::plan_execute::StaticPlanner;

let planner = StaticPlanner::new(vec![
    "Search for relevant documents",
    "Extract key information",
    "Write summary",
]);
```

### LlmPlanner

LLM-based dynamic planning:

```rust
use echo_agent::agent::plan_execute::LlmPlanner;

let planner = LlmPlanner::new("qwen3-max")
    .with_output_mode(PlannerOutputMode::Structured);  // or NaturalLanguage
```

---

## Executor Types

### SimpleExecutor

Wraps any Agent:

```rust
use echo_agent::agent::plan_execute::SimpleExecutor;

let agent = ReactAgentBuilder::simple("qwen3-max", "Executor")?;
let executor = SimpleExecutor::new(agent);
```

### ReactExecutor

ReAct agent with tools:

```rust
use echo_agent::agent::plan_execute::ReactExecutor;

let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .name("executor")
    .system_prompt("Execute the given step")
    .enable_tools()
    .build()?;

let executor = ReactExecutor::new(agent);
```

---

## Usage

```rust
use echo_agent::prelude::*;
use echo_agent::agent::plan_execute::{PlanExecuteAgent, LlmPlanner, ReactExecutor};

#[tokio::main]
async fn main() -> Result<()> {
    let planner = LlmPlanner::new("qwen3-max");

    let executor_agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("executor")
        .system_prompt("You are a task executor")
        .enable_tools()
        .build()?;
    let executor = ReactExecutor::new(executor_agent);

    let mut agent = PlanExecuteAgent::new("plan_agent", planner, executor)
        .max_replans(3);

    let result = agent
        .execute("Research Rust async patterns and write a summary report")
        .await?;

    println!("{}", result);
    Ok(())
}
```

---

## Execution Modes

### Sequential (default)

Steps execute one-by-one in dependency order:

```rust
let agent = PlanExecuteAgent::new("planner", planner, executor)
    .with_execution_mode(ExecutionMode::Sequential);
```

### Parallel

Independent steps run concurrently:

```rust
let execute_fn: TaskExecuteFn = Arc::new(|ctx| {
    Box::pin(async move { Ok(format!("done: {}", ctx.task_id)) })
});

let agent = PlanExecuteAgent::new("planner", planner, executor)
    .with_execute_fn(execute_fn)
    .max_concurrent(5);
```

---

## Incremental Replanning

When a step fails, only affected downstream steps are replanned:

```rust
// step_2 fails → identify downstream: [step_3, step_4]
// Remove affected tasks from TaskManager
// Generate new plan for affected subgraph
// Continue execution
```

This avoids redoing already-completed work.

---

## Plan Validation

Plans are automatically validated and fixed:

```rust
let issues = plan.validate();
for issue in &issues {
    match issue.severity {
        IssueSeverity::Error => warn!("Plan error: {}", issue.message),
        IssueSeverity::Warning => info!("Plan warning: {}", issue.message),
    }
}
plan.auto_fix();  // Fix common issues
```

---

## Streaming Events

```rust
let mut stream = agent.execute_stream(task).await?;

while let Some(event) = stream.next().await {
    match event? {
        AgentEvent::PlanGenerated { steps } => {
            println!("Plan created with {} steps", steps.len());
        }
        AgentEvent::StepStart { step_index, description } => {
            println!("Step {}: {}", step_index + 1, description);
        }
        AgentEvent::StepEnd { step_index, success } => {
            println!("Step {} {}", step_index + 1, if success { "✓" } else { "✗" });
        }
        AgentEvent::FinalAnswer(answer) => {
            println!("Result: {}", answer);
            break;
        }
        _ => {}
    }
}
```

---

## When to Use

| Scenario | Suitability | Reason |
|----------|-------------|--------|
| Structured multi-step tasks | ★★★★★ | Explicit plan, predictable |
| DAG workflows | ★★★★★ | Topological scheduling |
| Need failure recovery | ★★★★☆ | Incremental replanning |
| Open-ended exploration | ★★☆☆☆ | Hard to pre-plan |

---

## vs ReAct

| Dimension | ReAct | Plan-and-Execute |
|-----------|-------|------------------|
| Planning | Ad-hoc per step | Explicit upfront |
| Parallelism | No | Yes (independent steps) |
| Replanning | Implicit retry | Incremental subgraph |
| Debuggability | Medium | High (visible plan) |
| Token cost | Medium | Higher (planning phase) |

See: `examples/demo22_plan_execute.rs`