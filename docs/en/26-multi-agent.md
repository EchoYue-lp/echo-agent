# Multi-Agent Orchestration — SubAgent and TeamAgent

## Overview

echo-agent provides two multi-agent patterns:

1. **SubAgent** — Parent-child delegation with 3 execution modes (Sync, Fork, Teammate)
2. **TeamAgent** — Peer collaboration with 4 strategies (ManagerSubagent, Pipeline, Debate, Swarm)

Both are feature-gated behind `subagent`.

```
┌─────────────────────────────────────────────────────────┐
│                   Multi-Agent Patterns                   │
│                                                         │
│  ┌─────────────────────┐  ┌──────────────────────────┐  │
│  │      SubAgent        │  │       TeamAgent          │  │
│  │  (parent → child)    │  │  (peer ↔ peer)           │  │
│  │                      │  │                          │  │
│  │  • Sync (blocking)   │  │  • ManagerSubagent         │  │
│  │  • Fork (independent)│  │  • Pipeline              │  │
│  │  • Teammate (handle) │  │  • Debate                │  │
│  │                      │  │  • Swarm                 │  │
│  └─────────────────────┘  └──────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

---

## Feature Gate

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["subagent"] }
```

---

## SubAgent — Parent-Child Delegation

SubAgent is the simpler pattern: a parent agent dispatches tasks to child agents.

### Execution Modes

| Mode | Context Inheritance | Communication | Use Case |
|------|-------------------|---------------|----------|
| **Sync** | None (shared state via mutex) | Return value | Simple delegation, blocking wait |
| **Fork** | System prompt + tools + recent history | Return value | Independent subtask with parent context |
| **Teammate** | None | Join/cancel handle | Parallel independent work |

### Registration

```rust
use echo_agent::prelude::*;
use echo_agent::agent::subagent::SubagentBuilder;

let parent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("You are a coordinator")
    .enable_subagent()
    .subagent(
        SubagentBuilder::new("code-explorer")
            .description("Explores and reads code files")
            .model("qwen3-max")
            .system_prompt("You are a code exploration expert")
            .build()
    )
    .subagent(
        SubagentBuilder::new("web-researcher")
            .description("Searches the web for information")
            .model("qwen3-max")
            .system_prompt("You are a web research expert")
            .build()
    )
    .build()?;
```

### Dispatch Tool

When `enable_subagent()` is called, the parent agent gets an `agent_dispatch` tool automatically. The LLM can call it to delegate tasks:

```
User: "Read src/main.rs and find related documentation online"
  → Agent calls agent_dispatch("code-explorer", "Read src/main.rs")
  → Agent calls agent_dispatch("web-researcher", "Find documentation for the patterns in src/main.rs")
  → Agent synthesizes results
```

### Context Isolation

Each SubAgent runs in its own context. By default:
- No memory sharing (each has its own Store / runtime state)
- No tool sharing (each has its own ToolManager)
- No history sharing (each has its own message list)

This prevents context contamination between agents.

---

## TeamAgent — Peer Collaboration

TeamAgent is the advanced pattern: multiple agents collaborate as peers under a strategy.

### Team Roles

| Role | Responsibility |
|------|---------------|
| **Leader** | Decomposes tasks, assigns work, synthesizes results |
| **Subagent** | Executes assigned subtasks |
| **Reviewer** | Validates outputs (optional) |

### Four Collaboration Strategies

#### 1. ManagerSubagent (default)

The leader decomposes the task, fans out to subagents, and synthesizes the result.

```
           ┌─────────┐
           │ Manager  │
           └────┬────┘
        ┌───────┼───────┐
        ▼       ▼       ▼
   ┌────────┐┌────────┐┌────────┐
   │Subagent 1││Subagent 2││Subagent 3│
   └────┬───┘└────┬───┘└────┬───┘
        └───────┬─┘─────────┘
                ▼
           ┌─────────┐
           │ Manager  │ (synthesize)
           └─────────┘
```

```rust
use echo_agent::agent::subagent::team::{TeamAgent, TeamAgentBuilder, TeamStrategy};

let team = TeamAgentBuilder::new()
    .model("qwen3-max")
    .strategy(TeamStrategy::ManagerSubagent)
    .member("researcher", "Search for relevant information", TeamRole::Subagent)
    .member("analyst", "Analyze the findings", TeamRole::Subagent)
    .member("writer", "Write the final report", TeamRole::Subagent)
    .build()?;

let result = team.execute("Write a report about Rust async patterns").await?;
```

#### 2. Pipeline

Agents run in sequence: each agent's output becomes the next agent's input.

```
┌──────────┐    ┌──────────┐    ┌──────────┐
│ Agent 1  │───▶│ Agent 2  │───▶│ Agent 3  │
│ (research)│   │ (analyze) │   │ (write)  │
└──────────┘    └──────────┘    └──────────┘
```

```rust
let team = TeamAgentBuilder::new()
    .model("qwen3-max")
    .strategy(TeamStrategy::Pipeline(vec![
        "researcher".into(),
        "analyst".into(),
        "writer".into(),
    ]))
    .member("researcher", "Research the topic", TeamRole::Subagent)
    .member("analyst", "Analyze the research", TeamRole::Subagent)
    .member("writer", "Write the final output", TeamRole::Subagent)
    .build()?;
```

#### 3. Debate

Multiple agents independently propose solutions. A judge selects the best one.

```
┌──────────┐  ┌──────────┐  ┌──────────┐
│Debater 1 │  │Debater 2 │  │Debater 3 │
│(propose) │  │(propose) │  │(propose) │
└────┬─────┘  └────┬─────┘  └────┬─────┘
     └─────────────┼─────────────┘
                   ▼
            ┌──────────┐
            │  Judge   │ (select best)
            └──────────┘
```

```rust
let team = TeamAgentBuilder::new()
    .model("qwen3-max")
    .strategy(TeamStrategy::Debate {
        judge: "judge".into(),
        debaters: vec!["architect-a".into(), "architect-b".into()],
    })
    .member("judge", "Evaluate proposals and select the best", TeamRole::Reviewer)
    .member("architect-a", "Propose architecture A", TeamRole::Subagent)
    .member("architect-b", "Propose architecture B", TeamRole::Subagent)
    .build()?;
```

#### 4. Swarm

Work is split across agents by module/file. Each agent inspects its portion, then a reducer merges findings.

```
┌──────────┐  ┌──────────┐  ┌──────────┐
│Subagent 1  │  │Subagent 2  │  │Subagent 3  │
│(src/a/)  │  │(src/b/)  │  │(src/c/)  │
└────┬─────┘  └────┬─────┘  └────┬─────┘
     └─────────────┼─────────────┘
                   ▼
            ┌──────────┐
            │ Reducer  │ (merge findings)
            └──────────┘
```

```rust
let team = TeamAgentBuilder::new()
    .model("qwen3-max")
    .strategy(TeamStrategy::Swarm {
        batch_size: 3,
        reducer: "synthesizer".into(),
    })
    .member("subagent-1", "Analyze files in src/agent/", TeamRole::Subagent)
    .member("subagent-2", "Analyze files in src/tools/", TeamRole::Subagent)
    .member("subagent-3", "Analyze files in src/memory/", TeamRole::Subagent)
    .member("synthesizer", "Merge all findings into a report", TeamRole::Reviewer)
    .build()?;
```

---

## SubAgent vs TeamAgent

| Aspect | SubAgent | TeamAgent |
|--------|----------|-----------|
| Relationship | Parent-child | Peer-to-peer |
| Direction | One-way dispatch | Bidirectional collaboration |
| Context | Isolated (no sharing) | Isolated member executions |
| Coordination | Parent decides | Strategy-driven |
| Complexity | Simple | Advanced |
| Use case | Tool-like delegation | Complex multi-step workflows |
| Feature gate | `subagent` | `subagent` |

### When to Use Which

- **SubAgent**: When you need to delegate specific tasks to specialized agents (like calling a tool). The parent knows exactly what to ask.
- **TeamAgent**: When you need agents to collaborate on a complex task that requires decomposition, parallel execution, or debate.

---

## Execution Lifecycle

Teammate dispatch returns a handle that owns join and cancellation. TeamAgent
member execution uses the same canonical Subagent dispatcher, so lifecycle
events, isolation, timeout, cancellation, usage, and terminal status are not
reimplemented by a separate team protocol.

---

## Configuration

```rust
TeamConfig {
    max_concurrent: 5,           // Max concurrent subagents
    default_timeout_secs: 600,   // Aggregate team execution timeout
}
```

---

See also: [06 - SubAgent Orchestration](./06-subagent.md) for the original SubAgent documentation.
