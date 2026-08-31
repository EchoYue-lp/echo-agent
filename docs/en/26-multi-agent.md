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

Active Subagent messages use `SubagentExecutor::send_message_tracked`. Its
`SubagentMessageReceipt` contains the exact attempt identity and the nested
`AgentSteerReceipt`; the nested receipt is the only authority for mailbox
acceptance, context drain, and owning-turn settlement. A turn ID alone is not a
delivery-complete result.

`SubagentAttemptIdentity` is a framework-owned, serializable value containing
only the logical task, physical execution, and attempt number. Persist this
value directly when a consumer needs to correlate a command or recovery record;
no product identity mirror is required. Deserialization enforces the same
non-empty task/execution and positive-attempt invariants as `new`.
Unknown identity fields are rejected as well.

`SubagentResult::usage()` returns the serializable `ExecutionUsage`
facts common to every consumer: duration, total reported tokens, and iteration
count. Products may project this value into their UI, but should not redefine
the Rust usage model.

For durable control commands, use `SubagentCommandIdentity` directly. It adds
the generic run, plan-revision, and idempotency fields around the exact attempt
identity and validates the complete command envelope.
Its `SubagentCommandPhase` is the durable lifecycle (`persisted`,
`mailbox_accepted`, `drained`, `turn_settled`); the in-process binding remains
the separate `SubagentControlPhase`.

## Team Intent

`TeamSpec` contains registered Subagent names only. It does not hold Agent
instances, a relationship store, or a scheduler. This is the preferred entry
point when the application already owns a `SubagentRegistry`.

```rust
use echo_agent::prelude::*;

let definition = SubagentBuilder::new("review-team")
    .description("Review a change from independent perspectives")
    .team(TeamSpec {
        strategy: TeamStrategy::ManagerSubagent,
        manager: "review-lead".to_string(),
        subagents: vec!["correctness".to_string(), "tests".to_string()],
        config: TeamConfig {
            max_concurrent: 2,
            ..TeamConfig::default()
        },
    })
    .build();

assert_eq!(definition.name, "review-team");
```

Register the Team definition and every referenced member in the same
`SubagentRegistry`. Dispatch the Team definition with `ExecutionMode::Team`, or
invoke `agent_tool` with `mode: "team"`.

Framework consumers can also compose existing Agent objects without creating a
second execution path:

```rust,ignore
use echo_agent::prelude::*;

let team = TeamAgent::builder()
    .name("review-team")
    .manager("lead", lead_agent, lead_definition)
    .subagent("correctness", correctness_agent, correctness_definition)
    .subagent("tests", tests_agent, tests_definition)
    .strategy(TeamStrategy::ManagerSubagent)
    .build()?;

let answer = team.execute("Review the current change").await?;
```

The builder retains object identity for the framework consumer, but registers
the objects into one shared `SubagentRegistry` on first execution. Member work
therefore still passes through `SubagentExecutor` hooks, prompt compilation,
cancellation, typed result parsing, and usage accounting.

The strategies compile to ordinary task dependencies:

| Strategy | Canonical graph |
|---|---|
| `ManagerSubagent` | manager plan task -> revision patch -> member tasks -> manager synthesis |
| `Pipeline(names)` | one dependency chain; each output becomes the next task payload unchanged |
| `Debate { judge, debaters }` | parallel proposals -> judge synthesis |
| `Swarm { reducer }` | declared member shards -> reducer synthesis |

The manager must return a typed JSON task plan. Unknown fields, unknown
Subagents, duplicate task IDs, and invalid dependencies fail closed before the
graph revision is committed. Manager, debate, and swarm dependencies are included in the dependent
Subagent's task prompt. Pipeline preserves its stronger contract: the previous
output becomes the next member's complete task payload before the shared
`SubagentExecutor` applies its normal invocation context and prompt policy. The
framework does not infer a second status from free-form model text: the canonical
`SubagentResult.outcome.status` settles each task claim.

## Runtime Authority

The production flow is:

```text
TeamSpec or TeamAgentBuilder
  -> TeamRuntime (default in-memory or caller-owned persistent adapter)
  -> TaskRevisionService
  -> RuntimeTaskService
  -> SubagentExecutor
  -> typed SubagentResult
  -> exact claim settlement in the same revisioned graph
```

`RuntimeTaskService` exclusively owns ready-frontier traversal, derived dependency
blocking, bounded waves, cancellation, and terminal outcome selection. Team
code only compiles intent and supplies a thin dispatch adapter. ReAct
checkpoints do not duplicate task nodes or task lifecycle state.

With an explicit `run_id`, `TeamAgent` retains its default in-memory runtime so
executing the same object resumes the existing graph without redispatching
completed members. Without a stable run ID, each call uses a short-lived runtime
and does not accumulate anonymous graphs. Persistent framework consumers
implement `TeamRuntime` over their existing revision store and typed result
authority, then call `execute_team_on_runtime`.
A runtime must persist each successful `SubagentResult` before exposing that
task as Completed. The stored objective and serialized `TeamSpec` are checked
on every resume; reusing a run id for different intent fails closed. A product
task policy may extend task metadata, but must preserve Team-owned metadata keys
and may not rewrite dependency, member, or execution semantics.

## Design References

This split follows two mature orchestration patterns:

- [OpenAI Agents SDK multi-agent orchestration](https://openai.github.io/openai-agents-python/multi_agent/)
  keeps concrete specialist Agents composable while one Runner owns execution.
- [LangGraph supervisor](https://github.com/langchain-ai/langgraph-supervisor-py)
  accepts pre-built agents but compiles their collaboration into one graph with
  one checkpointer/store boundary.

echo-agent therefore preserves both name-based and object-based composition,
while revision, ready-frontier, retry, cancellation, and settlement remain in
the canonical task runtime. The removed Team-specific `TaskNode` loop is not
restored.

## Choosing A Mode

- Use `Sync` for one focused call whose result is immediately required.
- Use `Fork` for one isolated call with explicit context transfer.
- Use `Teammate` when the caller needs a live join/cancel handle.
- Use `Team` when collaboration has explicit member dependencies and a final
  synthesis step.
