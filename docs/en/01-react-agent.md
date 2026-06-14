# ReAct Agent — Core Execution Engine

## What It Is

ReAct (**Re**asoning + **Act**ing) is the most widely adopted Agent execution paradigm. Each iteration follows three steps:

```
Thought (reasoning) → Action (tool call) → Observation (result)
```

This loop repeats until the LLM determines the task is complete and calls the `final_answer` tool.

`ReactAgent` is the core implementation in echo-agent. It integrates tool management, memory, context compression, human-in-the-loop, SubAgent orchestration, and streaming output into a single cohesive structure.

---

## Problem It Solves

A bare LLM call is one-shot: given an input, return an output. This cannot handle tasks requiring multi-step reasoning, external tool access, or dynamic decision-making.

ReAct solves:
- **Reasoning-action separation**: LLM thinks before acting, enabling arbitrarily complex tasks
- **Tool invocation**: Execute code, query databases, call APIs
- **Iterative error correction**: Adjust strategy when tools return errors
- **Chain-of-Thought**: Naturally produces a traceable reasoning trail for debugging

---

## Execution Flow

```
execute(task)
    │
    ├─ 1. Load runtime state (RuntimeStateStore)
    ├─ 2. Inject long-term memories (Store)
    │
    └─ Loop (up to max_iterations):
          │
          ├─ context.prepare()     ← auto-compress if over token_limit
          │
          ├─ llm.chat()            ← call LLM
          │
          ├─ Parse response:
          │     ├─ content present → Token event (CoT reasoning text)
          │     └─ tool_calls      → tool call list
          │
          ├─ Execute all tool calls in parallel:
          │     ├─ Human approval check (if tool is marked)
          │     ├─ ToolManager.execute_tool()
          │     └─ Fire on_tool_start / on_tool_end callbacks
          │
          ├─ final_answer called → return result, exit loop
          │
          └─ Append assistant + tool_results messages to context

    └─ Save runtime state (RuntimeStateStore)
```

---

## Agent Roles

`AgentRole` controls the execution mode:

| Role | Description |
|------|-------------|
| `Worker` (default) | Directly executes tasks using its tools |
| `Orchestrator` | Delegates sub-tasks to SubAgents via `agent_tool` |

> **Note:** Task planning is enabled via `.enable_task(true)` (registers `plan`/`create_task`/`update_task` tools). No separate `Planner` role needed.

---

## Key Configuration

```rust
AgentConfig::new("qwen3-max", "my_agent", "You are a helpful assistant")
    .enable_tool(true)          // enable tool calling (default: true)
    .enable_task(true)          // enable DAG task planning
    .enable_subagent(true)      // enable SubAgent dispatch (Orchestrator mode)
    .enable_memory(true)        // enable long-term memory (Store + remember/recall/forget tools)
    .enable_human_in_loop(true) // enable human approval gate
    .enable_cot(true)           // enable Chain-of-Thought prompt injection (Builder default: true)
    .session_id("thread-001")   // logical run-grouping label
    .conversation_id("conv-001")// transcript ID, also keys RuntimeStateStore restore
    .token_limit(8192)          // context token limit (auto-compress when exceeded)
    .max_iterations(30)         // max iterations (prevents infinite loops)
```

---

## Lifecycle Callbacks

Implement `AgentCallback` to observe every phase of execution (for analytics, logging, UI updates, etc.):

```rust
use echo_agent::agent::{AgentCallback, AgentEvent};
use echo_agent::llm::types::Message;
use futures::future::BoxFuture;
use serde_json::Value;

struct MyCallback;

impl AgentCallback for MyCallback {
    fn on_think_start<'a>(&'a self, agent: &'a str, messages: &'a [Message]) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            println!("[{}] Thinking with {} messages in context", agent, messages.len());
        })
    }

    fn on_tool_start<'a>(&'a self, agent: &'a str, tool: &'a str, args: &'a Value) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            println!("[{}] Calling tool: {} {:?}", agent, tool, args);
        })
    }

    fn on_tool_end<'a>(&'a self, agent: &'a str, tool: &'a str, result: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            println!("[{}] Tool result: {} -> {}", agent, tool, &result[..result.len().min(80)]);
        })
    }

    fn on_final_answer<'a>(&'a self, agent: &'a str, answer: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
        println!("[{}] Final answer: {}", agent, answer);
    }
}
```

---

## Minimal Demo

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("qwen3-max", "assistant", "You are a helpful assistant");
    let mut agent = ReactAgent::new(config);

    let answer = agent.execute("What is 1 + 1?").await?;
    println!("{}", answer);
    Ok(())
}
```

---

## Full Demo (with tools + callback)

```rust
use echo_agent::prelude::*;
use echo_agent::tools::others::math::{AddTool, MultiplyTool};
use std::sync::Arc;

struct LogCallback;

#[async_trait::async_trait]
impl AgentCallback for LogCallback {
    async fn on_tool_start(&self, agent: &str, tool: &str, args: &serde_json::Value) {
        println!("  [{}] Calling {} args={}", agent, tool, args);
    }
    async fn on_final_answer(&self, _agent: &str, answer: &str) {
        println!("Final answer: {}", answer);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new(
        "qwen3-max",
        "math_agent",
        "You are a math assistant. Use tools to calculate.",
    )
    .enable_tool(true)
    .max_iterations(10);

    let mut agent = ReactAgent::new(config);
    agent.add_tools(vec![Box::new(AddTool), Box::new(MultiplyTool)]);
    agent.add_callback(Arc::new(LogCallback));

    let answer = agent.execute("What is (3 + 4) * 5?").await?;
    println!("{}", answer);
    Ok(())
}
```

See: `examples/demo01_tools.rs`, `examples/demo11_callbacks.rs`

---

## Design Notes

**Why CoT text instead of a dedicated `think` tool?**

The old approach provided a `think` tool for the LLM to reason. The new approach appends `COT_INSTRUCTION` to the system prompt, letting the LLM output reasoning text in the `content` field before each tool call. Benefits:
1. Reasoning content is naturally part of message history (context)
2. Directly produces streaming Token events — UI can show thinking in real time
3. Eliminates one round-trip tool call

**Parallel tool calls**

When the LLM returns multiple tool calls in a single response, ReactAgent uses `join_all()` to execute them concurrently, bounded by `ToolExecutionConfig::max_concurrency`.
