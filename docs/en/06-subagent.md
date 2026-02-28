# Multi-Agent Orchestration (SubAgent)

## What It Is

Multi-Agent orchestration allows a main Agent (Orchestrator) to decompose a task and delegate parts to specialized SubAgents, then aggregate the results. Each Agent is an independent `ReactAgent` instance with its own context, tool set, memory, and system prompt.

---

## Problem It Solves

Limitations of a single monolithic Agent:
- **Capability boundaries**: One Agent can't simultaneously excel at math, creative writing, and weather queries
- **Context pollution**: Packing every tool and piece of knowledge into one Agent causes LLM confusion
- **Serial inefficiency**: Multiple independent sub-tasks run sequentially, wasting time
- **Security isolation**: Different tasks' contexts should not be visible to each other

Multi-Agent orchestration splits a "generalist" into multiple "specialists" coordinated by an Orchestrator, each doing what it does best.

---

## Three Agent Roles

```rust
AgentConfig::new(...).role(AgentRole::Orchestrator) // coordinator
AgentConfig::new(...).role(AgentRole::Worker)        // executor (default)
AgentConfig::new(...).role(AgentRole::Planner)       // task planner
```

| Role | Behavior |
|------|----------|
| `Orchestrator` | Receives user task → decomposes → dispatches via `agent_tool` → aggregates |
| `Worker` | Receives specific task → executes with its own tools → returns result |
| `Planner` | Receives complex task → generates DAG with `plan` tool → executes step by step |

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
| Tool set | Each SubAgent registers its own tools; Orchestrator's tools are invisible to SubAgents |
| Long-term memory | Each Agent uses `[agent_name, "memories"]` as an independent Store namespace |
| Short-term session | Each Agent has an independent `session_id`; Checkpointer stores per-session |

---

## Usage

```rust
use echo_agent::prelude::*;
use echo_agent::tools::others::math::{AddTool, MultiplyTool};
use echo_agent::tools::others::weather::WeatherTool;

// 1. Create specialized SubAgents
let math_agent = {
    let config = AgentConfig::new("gpt-4o", "math_agent", "You are a math expert")
        .enable_tool(true)
        .allowed_tools(vec!["add".into(), "multiply".into()]); // enforce tool boundaries
    let mut agent = ReactAgent::new(config);
    agent.add_tools(vec![Box::new(AddTool), Box::new(MultiplyTool)]);
    Box::new(agent) as Box<dyn Agent>
};

let weather_agent = {
    let config = AgentConfig::new("gpt-4o", "weather_agent", "You are a weather expert")
        .enable_tool(true)
        .allowed_tools(vec!["get_weather".into()]);
    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(WeatherTool));
    Box::new(agent) as Box<dyn Agent>
};

// 2. Create the main Orchestrator Agent
let main_config = AgentConfig::new(
    "gpt-4o",
    "orchestrator",
    "You are the main orchestrator. Use agent_tool to delegate:
     - math_agent: math calculations
     - weather_agent: weather queries
     Do NOT calculate or query directly yourself.",
)
.role(AgentRole::Orchestrator)
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

## SubAgent Dispatch Flow

```
main_agent.execute("...")
    │
    ├─ LLM decides to call agent_tool
    │      { "agent_name": "math_agent", "task": "Calculate 25 * 3" }
    │
    ├─ AgentDispatchTool::execute()
    │      ├─ Find "math_agent" in the subagents HashMap
    │      ├─ Lock (AsyncMutex — serializes concurrent calls to same SubAgent)
    │      └─ math_agent.execute("Calculate 25 * 3")
    │              ├─ Runs with its own independent context
    │              ├─ Uses its own tools (add/multiply)
    │              └─ Returns "75"
    │
    └─ Tool result "75" appended to main Agent context
       LLM continues to reason and produce final answer
```

---

## Concurrent SubAgent Calls

When the main Agent dispatches to multiple **different** SubAgents in a single LLM response (multiple tool_calls), the framework executes them in parallel:

```
LLM returns in one response:
    agent_tool("math_agent",    "Compute A")  ┐
    agent_tool("weather_agent", "Get weather") ┤  parallel (join_all)
```

Concurrent calls to the **same SubAgent** are serialized by `AsyncMutex` to maintain state consistency.

---

## Memory Isolation per SubAgent

```rust
// SubAgent with its own session and memory, fully isolated from the main Agent
let sub_config = AgentConfig::new("gpt-4o", "sub_a", "...")
    .session_id("sub-a-session-001")
    .checkpointer_path("./checkpoints.json") // same file, unique session_id
    .enable_memory(true)
    .memory_path("./store.json");            // same file, unique namespace
```

---

## Best Practices

1. **Set clear `allowed_tools` for each SubAgent** to prevent capability overreach
2. **Explicitly list each SubAgent's responsibility in the Orchestrator's system prompt** to guide correct dispatching
3. **Don't enable `enable_subagent(true)` on SubAgents** — avoid recursive nesting that's hard to debug
4. **Use Planner role + DAG task system for complex tasks** rather than relying on ad-hoc Orchestrator decisions

See: `examples/demo04_suagent.rs`, `examples/demo14_memory_isolation.rs`
