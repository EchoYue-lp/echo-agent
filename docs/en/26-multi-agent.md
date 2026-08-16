# Multi-Agent Orchestration

echo-agent has one Subagent dispatch surface and one task-relationship runtime.
Use `Sync`, `Fork`, or `Teammate` for one registered Subagent. Use `Team` when a
declarative collaboration intent should be compiled into a revisioned task DAG.

## Single-Subagent Modes

| Mode | Parent behavior | Context default |
|---|---|---|
| `Sync` | Waits for the result | Fresh focused context |
| `Fork` | Runs through an owned async dispatch | Explicit filtered history |
| `Teammate` | Returns a join/cancel handle | Fresh independent context |

Every mode resolves the target through `SubagentRegistry` and executes through
`SubagentExecutor`. Direct tool dispatch and programmatic dispatch therefore
share hooks, cancellation, prompt compilation, isolation, and typed events.

## Team Intent

`TeamSpec` contains registered Subagent names only. It does not hold Agent
instances, a relationship store, or a scheduler.

```rust
use echo_agent::agent::subagent::{
    SubagentBuilder, TeamConfig, TeamSpec, TeamStrategy,
};

let definition = SubagentBuilder::new("review-team")
    .description("Review a change from independent perspectives")
    .team(TeamSpec {
        strategy: TeamStrategy::ManagerSubagent,
        manager: "review-lead".to_string(),
        subagents: vec!["correctness".to_string(), "tests".to_string()],
        config: TeamConfig { max_concurrent: 2 },
    })
    .build();

assert_eq!(definition.name, "review-team");
```

Register the Team definition and every referenced member in the same
`SubagentRegistry`. Dispatch the Team definition with `ExecutionMode::Team`, or
invoke `agent_tool` with `mode: "team"`.

The strategies compile to ordinary task dependencies:

| Strategy | Canonical graph |
|---|---|
| `ManagerSubagent` | manager planning -> member tasks -> manager synthesis |
| `Pipeline(names)` | one dependency chain in the supplied order |
| `Debate { judge, debaters }` | parallel proposals -> judge synthesis |
| `Swarm { reducer }` | declared member shards -> reducer synthesis |

Completed dependency outputs are appended to the dependent Subagent's task
prompt. The framework does not infer a second status from free-form model text:
the canonical `SubagentResult.outcome.status` settles each task claim.

## Runtime Authority

The production flow is:

```text
TeamSpec
  -> TaskRevisionService + InMemoryRevisionedTaskStore
  -> RuntimeDagExecutor
  -> SubagentExecutor
  -> typed SubagentResult
  -> exact claim settlement in the same revisioned graph
```

`RuntimeDagExecutor` exclusively owns ready-frontier traversal, dependency
blocking, bounded waves, cancellation, and terminal outcome selection. Team
code only compiles intent and supplies a thin dispatch adapter. ReAct
checkpoints do not duplicate task nodes or task lifecycle state.

## Choosing A Mode

- Use `Sync` for one focused call whose result is immediately required.
- Use `Fork` for one isolated call with explicit context transfer.
- Use `Teammate` when the caller needs a live join/cancel handle.
- Use `Team` when collaboration has explicit member dependencies and a final
  synthesis step.
