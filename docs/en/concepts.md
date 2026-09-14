# Core Concepts

## Reading The Model

echo-agent uses qualified concepts instead of one global state object. Each
concept has an identity, a scope, an owner, and a persistence boundary. A name
such as Run, Revision, or Checkpoint is incomplete until its owner is known.

The framework can compose these concepts, but composition does not transfer
authority. For example, a Turn can emit a Trace and update a transcript without
making either record the Turn terminal authority.

## Identity Map

```text
Agent
  +-- Session / Conversation scope
        +-- Invocation
              +-- Turn ----> TurnReceipt
              +-- Trace Run (observation)
              +-- Tool Effect / Delivery attempt

Task revision
  +-- PlanTask claim
        +-- Subagent attempt ----> Subagent outcome

Context <- selected from transcript, checkpoint, memory, rules, and resources
```

Identities remain qualified across this map. A product Run ID, invocation
correlation, Turn ID, Task ID, Subagent attempt ID, and Trace Run ID are not
interchangeable even when one request connects them.

## Execution Concepts

| Concept | Meaning and owner | Not this |
| --- | --- | --- |
| `Agent` | A configured implementation of model preparation, context handling, the Tool loop, and raw execute/chat behavior | Not a registry for every Session, Task, or Trace |
| `Session` | A protocol or channel resource scope that can own multiple Turns and closeable resources | Not the model Context or transcript store |
| `Conversation` | A stable dialogue/history scope used by a Conversation store | Not one Invocation or one Turn |
| `Invocation` | One call's configuration, correlation, input, and resource guards | Not automatically the product Run or Trace Run |
| `Turn` | One admitted, cancellable driven execution whose terminal is represented by a TurnReceipt | Not a Task node or stream item |
| `Task` | A revisioned graph unit with specification, dependency, claim, execution, and settlement | Not a Todo row or progress event |
| `Plan` | A reviewable specification artifact compiled into or associated with Task revisions | Not an independent runtime state machine or store |
| `Subagent` | An Agent attempt with isolated Context, bounded capabilities, typed control, identity, and outcome | Not a second execution-role domain |

The lower-level `Agent` API remains a valid framework contract. Driven adapters
use the Turn lifecycle when they need acceptance, tracked input, cancellation,
and terminal receipts. See [ReAct Agent](./01-react-agent.md),
[Runtime and Tasks](./29-long-running-tasks.md), and [Subagent](./06-subagent.md).

## Context And Persistence Concepts

| Concept | Meaning and owner | Persistence and non-responsibility |
| --- | --- | --- |
| `Context` | The bounded messages and resources visible to the model for the current execution | Ephemeral/model-facing; not the complete transcript |
| `Checkpoint` | A recovery snapshot owned by a named runtime, workflow, or other state authority | Owner-scoped; not an ordered Journal or Git commit |
| `Store` / Memory | Application or Agent knowledge retained beyond the active Context | Scope depends on the Store; not execution terminal state |
| `Journal` | Ordered committed facts for a domain explicitly designed as journal-backed | Durable within that domain; not a global event bus |
| `Projection` | A query, history, feed, or UI view derived from facts or authoritative status | Rebuildable or consumer-owned; must not rewrite facts |
| `Trace` | Diagnostic observation of execution represented by a Trace Run and events | Retained by RunStore policy; not a business commit |
| `Delivery` | A routed payload attempt with lifecycle, acknowledgement, and settlement in a Delivery Ledger or explicit Outbox pattern | Tracks delivery; does not promise arbitrary effects exactly once |
| `Effect` | A file, process, network, Tool, or other externally visible action | Owned by its executor and cleanup path, not by its display event |

Context selection and compression are explained in [Context System](./40-context-system.md)
and [Compression](./04-compression.md). Durable meanings are separated in
[Persistence Concepts](./41-persistence-concepts.md), [Tracing](./27-tracing.md),
and [Delivery Ledger](./41-delivery-ledger.md).

## Qualified Revision, Run, And Checkpoint

| Qualified name | Owner | Meaning |
| --- | --- | --- |
| `TaskRevision` | TaskRevisionService | Immutable Task graph specification version |
| Plugin generation | Plugin publication lifecycle | One prepared and published component generation |
| Runtime-state incarnation | RuntimeStateStore scope | Resettable recovery lineage within a stable conversation scope |
| SDK schema Revision | SDK contract generator | Reproducible public inventory and protocol schema version |
| Trace Run | RunStore | Diagnostic execution record correlated to an Invocation or Turn |

There is no generic `AgentRevision` and no global Run that owns every lifecycle.
A Git or file Checkpoint is also separate from a runtime Checkpoint. Qualifying
these names prevents one module's version or recovery policy from silently
becoming another module's authority.

## State Authority Rules

1. One datum or transition has one authoritative owner.
2. Adapters convert identity, input, events, and errors without re-owning
   terminal state, Permission, or the Task graph.
3. A cancellation request is not a terminal. The owner publishes terminal only
   after its required producer and cleanup settlement.
4. EOF, final text, a Trace event, feed position, or UI rendering cannot prove
   successful terminal state by itself.
   Reply and Wait operations consume an authoritative result or receipt; they
   do not create a terminal by observing output.
5. Compression changes model-visible Context, not transcript or Journal facts.
6. `clear` and `reset` name the authority they affect; there is no implicit
   clear-everything operation.
7. Public API presence is a framework capability decision, not evidence that one
   application currently uses it.

## Framework Non-Ownership

| Concern | Framework role | Owner outside the framework |
| --- | --- | --- |
| Product Workspace | Accept an explicit invocation workspace reference and provide reusable primitives | Embedding application |
| Device synchronization | No generic state authority | Embedding application or platform |
| Product Backend and authentication | Expose protocol/runtime integration points | Product service |
| Frontend, Desktop, GUI, or TUI state | Emit typed events and results | Surface reducer and application lifecycle |
| Deployment and release policy | Provide binaries, libraries, telemetry, and diagnostics | Consumer delivery system |

The detailed placement rule is in [Framework and Application Boundary](./39-framework-application-boundary.md).

## Example Routes

| Concept path | Executable consumer |
| --- | --- |
| Agent and Tool loop | [`demo01_tools`](../../echo-agent-learning/examples/demo01_tools.rs) |
| Conversation and chat | [`demo17_chat`](../../echo-agent-learning/examples/demo17_chat.rs) |
| Context compression | [`demo53_adaptive_compression`](../../echo-agent-learning/tests/example_contracts/demo53_adaptive_compression.rs) |
| Task graph | [`demo02_tasks`](../../echo-agent-learning/examples/demo02_tasks.rs) |
| Subagent outcome | [`demo04_subagent`](../../echo-agent-learning/tests/example_contracts/demo04_subagent.rs) |
| MCP contract | [`demo30_mcp_server`](../../echo-agent-learning/tests/example_contracts/demo30_mcp_server.rs) |
| Eval and Trace consumer | [`demo50_eval`](../../echo-agent-learning/tests/example_contracts/demo50_eval.rs) |

These files are Cargo examples or test contracts. The documentation links to
their tested source instead of maintaining uncompiled copies.

## Further Reading

- [Framework Architecture](./architecture.md)
- [Framework Lifecycles](./lifecycles.md)
- [Task Planning](./09-tasks.md)
- [Context System](./40-context-system.md)
- [Persistence Concepts](./41-persistence-concepts.md)
- [ADR 0040](../adr/0040-framework-concept-documentation-authority.md)
