# ReAct Agent — Core Execution Engine

## What It Is

ReAct (**Re**asoning + **Act**ing) is the most widely adopted Agent execution paradigm. Each iteration follows three steps:

```
Thought (reasoning) → Action (tool call) → Observation (result)
```

完整原生响应形成候选后，继续经 Guard/Critic、Stop Hook、持久化和执行 owner 结算；消息 phase 不直接结束运行。

`ReactAgent` is the core implementation in echo-agent. It integrates tool management, memory, context compression, human-in-the-loop, Subagent orchestration, and streaming output into a single cohesive structure.

---

## Problem It Solves

A bare LLM call is one-shot: given an input, return an output. This cannot handle tasks requiring multi-step reasoning, external tool access, or dynamic decision-making.

ReAct solves:
- **Reasoning-action separation**: LLM thinks before acting, enabling arbitrarily complex tasks
- **Tool invocation**: Execute code, query databases, call APIs
- **Iterative error correction**: Adjust strategy when tools return errors
- **原生消息**：分别保留 provider reasoning 与普通 assistant 正文，不把普通文字推断为 thinking

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
          │     ├─ assistant blocks → 按原 phase 分别保留 reasoning 与正文
          │     └─ tool_calls      → tool call list
          │
          ├─ Execute all tool calls in parallel:
          │     ├─ Human approval check (if tool is marked)
          │     ├─ ToolManager.execute_tool()
          │     └─ Fire on_tool_start_with_id / on_tool_end_with_id callbacks
          │
          ├─ 原生最终候选 → 校验/Stop/持久化/结算
          │
          └─ Append assistant + tool_results messages to context

    └─ Save runtime state (RuntimeStateStore)
```

---

## Composable Capabilities

Agents do not use a separate role state machine. A regular `ReactAgent` becomes
an orchestrator by enabling Subagent dispatch and registering Subagents. The
revisioned `task_create`, `task_update`, and `task_list` tools provide planning
without changing the Agent's runtime role.

---

## Key Configuration

```rust
AgentConfig::new("qwen3-max", "my_agent", "You are a helpful assistant")
    .enable_tool(true)          // enable tool calling (default: true)
    .enable_subagent(true)      // enable Subagent dispatch (Orchestrator mode)
    .enable_memory(true)        // enable approved Store recall/search; manager adds writes
    .enable_human_in_loop(true) // enable human approval gate
    .session_id("thread-001")   // logical run-grouping label
    .conversation_id("conv-001")// transcript ID, also keys RuntimeStateStore restore
    .token_limit(8192)          // context token limit (auto-compress when exceeded)
    .max_iterations(30)         // max iterations (prevents infinite loops)
```

---

## Invocation-Scoped Tool Surface

Use `AgentInvocationContext::disabled_tools` when a caller needs to hide tools for
one turn without mutating a pooled/shared agent:

```rust
use echo_agent::agent::AgentInvocationContext;
use std::collections::HashSet;

let invocation = AgentInvocationContext {
    disabled_tools: Some(HashSet::from(["create_complex_task".to_string()])),
    ..Default::default()
};
```

The run snapshot combines these exclusions with agent-level defaults, then applies
the activated skill allowlist and plan-mode read-only surface. The result is frozen
for the invocation. Hidden tools are omitted from the model schema and rejected by
the execution pipeline if a provider still returns such a call.

`ReactAgent::set_disabled_tools` now sets defaults for subsequent runs; it does not
change snapshots that are already running.

For a shared revisioned policy across pooled Agents, use the framework
`ToolControlService`. It owns the explicit disabled-tool set and monotonic
mutation revision; apply its `ToolControlSnapshot::disabled_option()` to each
Agent or invocation. Product code may add registry checks and effective
visibility projections without creating a second policy authority.

### Run Budgets

`RunBudgetPolicy` adds opt-in convergence controls without changing the existing
hard `max_iterations` behavior. `iteration_wind_down_remaining` injects one short
instruction when the configured number of iterations remains. `max_model_tokens`
counts only provider-reported input and output tokens; missing usage remains unknown
and never triggers a fabricated threshold. Once the threshold is reached, the next
request exposes no tools and sends `tool_choice=none`. Decisions are emitted as
`AgentEvent::BudgetDecision` and recorded in the run trace.

Configure an agent default with `ReactAgentBuilder::run_budget`, or override one
call with `AgentInvocationContext::run_budget`. The run snapshot freezes the
resolved value, so queued invocations cannot change each other's budget.

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

    fn on_tool_start_with_id<'a>(&'a self, agent: &'a str, call_id: &'a str, tool: &'a str, args: &'a Value) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            println!("[{}] 调用工具: {} call_id={call_id} {:?}", agent, tool, args);
        })
    }

    fn on_tool_end_with_id<'a>(&'a self, agent: &'a str, call_id: &'a str, tool: &'a str, result: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let preview: String = result.chars().take(80).collect();
            println!("[{}] 工具结果: {} call_id={call_id} -> {}", agent, tool, preview);
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

#[echo_agent::callback]
impl LogCallback {
    async fn on_tool_start_with_id(&self, agent: &str, call_id: &str, tool: &str, args: &serde_json::Value) {
        println!("  [{}] 调用工具 {} call_id={call_id} args={}", agent, tool, args);
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

See: `echo-agent-learning/examples/demo01_tools.rs`, `echo-agent-learning/examples/demo11_callbacks.rs`

---

## Design Notes

**原生消息与控制工具退役**

原生 reasoning 与 assistant 正文使用独立 typed 块；不再注册 Think/FinalAnswer 控制工具，也不注入工具前强制分析提示。普通文字不因同响应调用工具而成为 thinking；最终发布须经原 Guard/Critic/Stop、持久化与物理结算。这里的运行时结算不等于应用 TaskRun 或 Goal 已完成。块身份、来源、阶段、增量与运行时最后终态见[原生消息合同](../zh/43-native-message-execution.md)。


**Parallel tool calls**

When the LLM returns multiple tool calls in a single response, ReactAgent uses `join_all()` to execute them concurrently, bounded by `ToolExecutionConfig::max_concurrency`.
