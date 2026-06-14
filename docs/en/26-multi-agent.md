# Multi-Agent Orchestration — SubAgent and TeamAgent

## Overview

echo-agent provides two multi-agent patterns:

1. **SubAgent** — Parent-child delegation with 3 execution modes (Sync, Fork, Teammate)
2. **TeamAgent** — Peer collaboration with 4 strategies (ManagerWorker, Pipeline, Debate, Swarm)

Both are feature-gated behind `subagent`.

```
┌─────────────────────────────────────────────────────────┐
│                   Multi-Agent Patterns                   │
│                                                         │
│  ┌─────────────────────┐  ┌──────────────────────────┐  │
│  │      SubAgent        │  │       TeamAgent          │  │
│  │  (parent → child)    │  │  (peer ↔ peer)           │  │
│  │                      │  │                          │  │
│  │  • Sync (blocking)   │  │  • ManagerWorker         │  │
│  │  • Fork (independent)│  │  • Pipeline              │  │
│  │  • Teammate (mailbox)│  │  • Debate                │  │
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
| **Teammate** | None | Mailbox (async mpsc) | Parallel independent work |

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
| **Worker** | Executes assigned subtasks |
| **Reviewer** | Validates outputs (optional) |

### Four Collaboration Strategies

#### 1. ManagerWorker (default)

The leader decomposes the task, fans out to workers, and synthesizes the result.

```
           ┌─────────┐
           │ Manager  │
           └────┬────┘
        ┌───────┼───────┐
        ▼       ▼       ▼
   ┌────────┐┌────────┐┌────────┐
   │Worker 1││Worker 2││Worker 3│
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
    .strategy(TeamStrategy::ManagerWorker)
    .member("researcher", "Search for relevant information", TeamRole::Worker)
    .member("analyst", "Analyze the findings", TeamRole::Worker)
    .member("writer", "Write the final report", TeamRole::Worker)
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
    .member("researcher", "Research the topic", TeamRole::Worker)
    .member("analyst", "Analyze the research", TeamRole::Worker)
    .member("writer", "Write the final output", TeamRole::Worker)
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
    .member("architect-a", "Propose architecture A", TeamRole::Worker)
    .member("architect-b", "Propose architecture B", TeamRole::Worker)
    .build()?;
```

#### 4. Swarm

Work is split across agents by module/file. Each agent inspects its portion, then a reducer merges findings.

```
┌──────────┐  ┌──────────┐  ┌──────────┐
│Worker 1  │  │Worker 2  │  │Worker 3  │
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
    .member("worker-1", "Analyze files in src/agent/", TeamRole::Worker)
    .member("worker-2", "Analyze files in src/tools/", TeamRole::Worker)
    .member("worker-3", "Analyze files in src/memory/", TeamRole::Worker)
    .member("synthesizer", "Merge all findings into a report", TeamRole::Reviewer)
    .build()?;
```

---

## SubAgent vs TeamAgent

| Aspect | SubAgent | TeamAgent |
|--------|----------|-----------|
| Relationship | Parent-child | Peer-to-peer |
| Direction | One-way dispatch | Bidirectional collaboration |
| Context | Isolated (no sharing) | Isolated (mailbox communication) |
| Coordination | Parent decides | Strategy-driven |
| Complexity | Simple | Advanced |
| Use case | Tool-like delegation | Complex multi-step workflows |
| Feature gate | `subagent` | `subagent` |

### When to Use Which

- **SubAgent**: When you need to delegate specific tasks to specialized agents (like calling a tool). The parent knows exactly what to ask.
- **TeamAgent**: When you need agents to collaborate on a complex task that requires decomposition, parallel execution, or debate.

---

## Mailbox Communication

TeamAgent members communicate via async mailboxes (tokio::sync::mpsc):

```rust
pub enum MessageKind {
    TaskAssigned { task_id: String, task: String },
    TaskResult { task_id: String, result: String },
    Query { question: String },
    QueryResponse { answer: String },
    Status { status: String },
    Cancelled { reason: String },
}
```

Each `TeamMember` gets a `Mailbox` with configurable capacity (default: 64 messages).

---

## Configuration

```rust
TeamConfig {
    max_concurrent: 5,           // Max concurrent workers
    default_timeout_secs: 300,   // 5 min timeout per task
    allow_reassignment: true,    // Reassign on failure
    cross_talk: false,           // Workers can't talk to each other
    mailbox_capacity: 64,        // Messages per mailbox
}
```

---

See also: [06 - SubAgent Orchestration](./06-subagent.md) for the original SubAgent documentation.
