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

`SubagentStatus` is a framework-owned lifecycle value. Parse its stable wire
spelling with the standard `status.parse()` API rather than a source-specific
helper.

This is the most critical property of a multi-Agent system. echo-agent guarantees it architecturally:

```
Main Agent system prompt = "Mission code PROJECT-OMEGA — strictly confidential..."
Main Agent conversation  = [system, user, assistant, ...]

    │ agent_tool("math_agent", "Calculate 7 * 8")
    ▼

math_agent.execute("Calculate 7 * 8")
    ↑
    Receives the compiled current Message; attachments are preserved
    Fresh mode receives no transcript; structured inheritance may add only
    filtered user messages and complete assistant final messages
    Parent system/tool/reasoning messages never transfer
    math_agent has a completely independent ContextManager instance
```

**`agent_tool` never transfers the parent system prompt or runtime trace.** The
default fresh path carries only the compiled current task message. Fork,
teammate, and team paths can explicitly inherit bounded structured history;
that history remains real messages rather than prompt text.

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

Every path first calls the configured `SubagentPromptCompiler`:

- `compile_system` runs after the concrete Subagent tool surface is built, so
  capability text comes from registered definitions, descriptions, and the
  current shared visibility policy.
- `SubagentDefinition::access_mode` is the typed read/write authority; tags are
  discovery metadata and are never decoded into an execution boundary.
- `compile_invocation` owns task, constraints, optional product payload,
  effective working directory, invocation tool allowlist, filtered structured
  history, and the current typed message including attachments.
- Sync, fork, teammate, and team execution consume the compiled messages
  directly; the executor neither appends a second prompt envelope nor rebuilds
  the current message afterward.

`ContextTransferPolicy::Fresh` carries no conversation transcript.
`InheritStructured` keeps only provider-safe user messages and complete
assistant final messages; system prompts, tool calls/results, reasoning, and
runtime projections are excluded.

```
main_agent.execute("...")
    │
    ├─ LLM decides to call agent_tool
    │      { "agent_name": "math_agent", "task": "Calculate 25 * 3" }
    │
    ├─ AgentDispatchTool::execute()
    │      ├─ Find "math_agent" in the subagents HashMap
    │      ├─ Lock (AsyncMutex — serializes concurrent calls to same Subagent)
    │      ├─ SubagentPromptCompiler::compile_invocation(...)
    │      └─ math_agent.execute(compiled messages)
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

See: `echo-agent-learning/tests/example_contracts/demo04_subagent.rs`

## Structured Outcome Views

`SubagentOutcome` is the framework-owned result contract. In addition to the
raw structured `evidence`, it exposes generic verification and file-access
views derived from that evidence:

```rust
let checks = outcome.verification();
let files = outcome.touched_files();
```

These views are part of the framework result and can be persisted or rendered
directly by an application. An application does not need to copy the outcome
into a second framework-shaped result type; only genuinely product-specific
wire fields should remain in the application.

`SubagentResult` is the execution envelope returned by a dispatch. It carries
runtime metadata such as the full output, duration, usage, mode, and the typed
`SubagentOutcome`. Lifecycle events expose that typed value as `outcome`, so
consumers do not have to infer whether a `result` field is the envelope or its
terminal payload.

`ExecutionUsage` is the canonical durable usage value for delegated Subagents
and finite primary-Agent turns. Both result surfaces expose it directly with
`result.usage()` or `turn_receipt.usage()`. Applications do not need a
source-named adapter, conversion trait, or parallel execution-usage DTO.

## Versioned Execution Events

`SubagentEventBus::subscribe_envelopes` is the authoritative execution-event
transport. Each outer dispatch attempt owns one `SubagentEventPublisher`; start,
isolation, displayable thinking/token deltas, usage, tool activity, and terminal
events share one `stream_id` and monotonic `sequence`. Internal hook retries do
not restart that sequence. `SubagentEventPayload::invocation` carries task,
attempt, plan revision, agent path, and parent-execution correlation without
parsing an execution-id string.

The existing `subscribe` method remains a raw compatibility view derived from
envelopes. Manual raw emission accepts registry events only and rejects execution
variants, so it cannot become a second execution authority. New consumers that
need ordering or recovery should use envelopes. Tool terminal envelopes
point to their matching tool-start event; other execution events point to the
dispatch start or an upstream parent event.

The broadcast and replay windows are bounded. A lagging receiver observes
Tokio's `Lagged` result, then calls `replay_after` for a known stream or
`replay_for_execution` for an exact attempt whose start envelope was missed.
`SubagentEventReplay::gap` explicitly reports when the retained suffix is not
contiguous. High-volume thinking/token deltas may be absent after a gap, while
retained lifecycle/tool boundaries and `terminal` allow state and final output
to be reconciled. This is an in-process recovery window, not permanent storage;
applications remain responsible for their own durable projections.
