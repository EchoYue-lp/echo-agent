# Multi-Agent Orchestration (Subagent)

## What It Is

Multi-Agent orchestration allows a main Agent (Orchestrator) to decompose a task and delegate parts to specialized Subagents, then aggregate the results. Each Agent is an independent `ReactAgent` instance with its own context, tool set, memory, and system prompt.

---

## Problem It Solves

Limitations of a single monolithic Agent:
- **Capability boundaries**: One Agent can't simultaneously excel at math, creative writing, and weather queries
- **Context pollution**: Packing every tool and piece of knowledge into one Agent causes LLM confusion
- **Serial inefficiency**: Multiple independent sub-tasks run sequentially, wasting time
- **Security isolation**: Different tasks' contexts should not be visible to each other

Multi-Agent orchestration splits a "generalist" into multiple "specialists" coordinated by an Orchestrator, each doing what it does best.

---

## Capability Composition

There is one `ReactAgent` runtime. A coordinating Agent enables Subagent
dispatch and registers specialized `ReactAgent` or `MockAgent` instances. Task
planning is an independent, revisioned tool capability rather than an Agent
role or a second execution engine.

---

## Context Isolation

This is the most critical property of a multi-Agent system. echo-agent guarantees it architecturally:

```
Main Agent system prompt = "Mission code PROJECT-OMEGA — strictly confidential..."
Main Agent conversation  = [system, user, assistant, ...]

    │ agent_tool("math_agent", "Calculate 7 * 8")
    ▼

math_agent.execute("Calculate 7 * 8")
    ↑
    Only receives this string — knows nothing about PROJECT-OMEGA
    math_agent has a completely independent ContextManager instance
```

**`agent_tool` passes only the task string — no context whatsoever.**

| Isolation Dimension | Guarantee |
|--------------------|-----------|
| Context (message history) | Each Agent is an independent `ReactAgent` Rust object — `ContextManager` has no shared references |
| Tool set | Each Subagent registers its own tools; Orchestrator's tools are invisible to Subagents |
| Long-term memory | Each Agent uses `[agent_name, "memories"]` as an independent Store namespace |
| Short-term session | Each Agent has an independent `conversation_id`; `RuntimeStateStore` tracks per-conversation state |

---

## Usage

```rust
use echo_agent::prelude::*;
use echo_agent::tools::others::math::{AddTool, MultiplyTool};
use echo_agent::tools::others::weather::WeatherTool;

// 1. Create specialized Subagents
let math_agent = {
    let config = AgentConfig::new("qwen3-max", "math_agent", "You are a math expert")
        .enable_tool(true)
        .allowed_tools(vec!["add".into(), "multiply".into()]); // enforce tool boundaries
    let mut agent = ReactAgent::new(config);
    agent.add_tools(vec![Box::new(AddTool), Box::new(MultiplyTool)]);
    Box::new(agent) as Box<dyn Agent>
};

let weather_agent = {
    let config = AgentConfig::new("qwen3-max", "weather_agent", "You are a weather expert")
        .enable_tool(true)
        .allowed_tools(vec!["get_weather".into()]);
    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(WeatherTool));
    Box::new(agent) as Box<dyn Agent>
};

// 2. Create the main Orchestrator Agent
let main_config = AgentConfig::new(
    "qwen3-max",
    "orchestrator",
    "You are the main orchestrator. Use agent_tool to delegate:
     - math_agent: math calculations
     - weather_agent: weather queries
     Do NOT calculate or query directly yourself.",
)
.enable_subagent(true)
.enable_tool(true);

let mut main_agent = ReactAgent::new(main_config);
main_agent.register_agents(vec![math_agent, weather_agent]);

// 3. Execute
let result = main_agent
    .execute("What's the weather in NYC? If it's above 68°F, calculate (68 + 5) * 2.")
    .await?;
println!("{}", result);
```

---

## Subagent Dispatch Flow

```
main_agent.execute("...")
    │
    ├─ LLM decides to call agent_tool
    │      { "agent_name": "math_agent", "task": "Calculate 25 * 3" }
    │
    ├─ AgentDispatchTool::execute()
    │      ├─ Find "math_agent" in the subagents HashMap
    │      ├─ Lock (AsyncMutex — serializes concurrent calls to same Subagent)
    │      └─ math_agent.execute("Calculate 25 * 3")
    │              ├─ Runs with its own independent context
    │              ├─ Uses its own tools (add/multiply)
    │              └─ Returns "75"
    │
    └─ Tool result "75" appended to main Agent context
       LLM continues to reason and produce final answer
```

---

## Concurrent Subagent Calls

When the main Agent dispatches to multiple **different** Subagents in a single LLM response (multiple tool_calls), the framework executes them in parallel:

```
LLM returns in one response:
    agent_tool("math_agent",    "Compute A")  ┐
    agent_tool("weather_agent", "Get weather") ┤  parallel (join_all)
```

Concurrent calls to the **same Subagent** are serialized by `AsyncMutex` to maintain state consistency.

---

## Memory Isolation per Subagent

```rust
// Subagent with its own session and memory, fully isolated from the main Agent
let sub_config = AgentConfig::new("qwen3-max", "sub_a", "...")
    .session_id("sub-a-session-001")
    .conversation_id("sub-a-conv-001")       // unique conversation_id for RuntimeStateStore
    .enable_memory(true)
    .memory_path("./store.json");            // same file, unique namespace
```

---

## Best Practices

1. **Set clear `allowed_tools` for each Subagent** to prevent capability overreach
2. **Explicitly list each Subagent's responsibility in the Orchestrator's system prompt** to guide correct dispatching
3. **Don't enable `enable_subagent(true)` on Subagents** — avoid recursive nesting that's hard to debug
4. **Use the revisioned task graph for complex task relationships** instead of prompt-only hidden state

See: `tests/example_contracts/demo04_subagent.rs`
