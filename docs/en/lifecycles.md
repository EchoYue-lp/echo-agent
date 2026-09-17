# Framework Lifecycles

## Lifecycle Invariants

Every lifecycle names its trigger, authority, visible events, external Effect,
cancellation/failure behavior, terminal boundary, recovery or cleanup owner,
and Projection. Similar status words do not make two lifecycles one state
machine.

- A cancellation request is not a terminal result.
- EOF, final text, Trace, feed position, and UI rendering do not prove success.
- Terminal publication follows the producer and cleanup settlement required by
  that owner.
- Retry and recovery reuse an operation identity or create an explicitly new
  generation; they do not silently replay an uncertain Effect.
- Retention removes eligible history, not live authority.

## Agent Turn

```text
surface request
  -> resolve Agent and Session resources
  -> assemble Invocation context
  -> admit and accept Turn
  -> Agent loop: model <-> Tool / Effect
     +-> owner-defined Trace / transcript / Checkpoint projections
  -> settle Agent-owned producers
  -> publish TurnReceipt terminal for a driven Turn
  -> adapter-owned projection and close follow the adapter contract
```

| Boundary | Contract |
| --- | --- |
| Trigger | Raw execute/chat, Headless, ACP prompt, Eval invocation, or another adapter |
| Admission | The driven Turn accepts input and establishes identity before work is reported as active |
| Authority | AgentTurnDriver and TurnReceipt for driven execution; raw Agent remains a lower-level API |
| Events and effects | Model output, Tool calls, tracked input, file/process/network effects, and owner-defined projections can occur during execution |
| Cancellation and failure | Cancellation is requested, producers are settled within their contract, and failure remains typed |
| Terminal | Completed, Failed, or Cancelled execution receipt plus an independent delivery result; no terminal is inferred from stream EOF |
| Recovery and projection | Session/runtime policy restores state; each Trace, transcript, checkpoint, sink, and adapter owns its write/close ordering |

Reply and Wait operations return or observe the owner's result or receipt. They
do not turn partial text, a notification, or an EOF into a new terminal fact.
Delivery failure is reported beside the execution terminal and cannot rewrite
an execution result already emitted by the producer.
Trace, transcript, and checkpoint writes can occur before or during finalization.
Their independently stored status and ordering do not define the TurnReceipt,
but a write error explicitly propagated by the Agent producer contract can make
a driven Turn fail. A best-effort transcript diagnostic does not do so by itself.
Adapter projection and awaited close coverage remain defined by each detailed
adapter contract, not by one fixed sequence in this overview.

See [ReAct Agent](./01-react-agent.md), [Streaming](./10-streaming.md), and
[Headless](./33-headless-mode.md). Adapter-specific guarantees stay in their
own protocol chapters; this page does not claim that every adapter already uses
the driven Turn path.

## Context And Persistence

```text
stable Conversation scope
  -> select runtime-state incarnation
  -> hydrate transcript and Checkpoint
  -> select / budget / assemble Context
  -> optionally compress model-visible messages
  -> execute Turn
  -> independently attempt transcript Projection and runtime Checkpoint writes
  -> report each owner-specific outcome
  -> clear or prune one named authority
```

| Operation | Authority affected | What remains |
| --- | --- | --- |
| Reset active Context | Agent/ContextManager | Durable transcript, runtime state, long-term Memory, and Trace unless explicitly changed |
| Clear runtime incarnation | RuntimeStateStore | Stable Conversation history and long-term Memory |
| Delete transcript | ConversationStore | Runtime or Memory data owned elsewhere |
| Delete long-term Memory | The selected Store | Transcript, Checkpoint, and Trace |
| Prune Checkpoint or Trace history | The named retention owner | Live state and other persistence domains |

Compression changes what the model sees; it does not rewrite transcript or
Journal facts. See [Context System](./40-context-system.md),
[Compression](./04-compression.md), [Memory](./03-memory.md), and
[Persistence Concepts](./41-persistence-concepts.md).
Checkpoint and transcript writes are separate owner commits. A runtime
Checkpoint can succeed while the current transcript Projection reports only a
best-effort persistence failure; one result must not be inferred from the other.

## Task And Subagent

```text
commit Task revision
  -> compute ready frontier
  -> claim PlanTask
  -> dispatch Subagent attempt
  -> observe progress and Effects
  -> receive Subagent outcome
  -> settle / retry / pause / cancel Task
  -> project Todo, event, and UI views
```

| Boundary | Contract |
| --- | --- |
| Trigger | Task create/update/execute or an orchestration request |
| Admission | A revision and claim fence the specification selected for execution |
| Authority | TaskRevisionService owns the graph; RuntimeTaskService owns dependency execution; Subagent executor owns the attempt |
| Events and effects | Task progress and Subagent envelopes observe execution; Tools own their external Effects |
| Cancellation and failure | The owning runtime settles the claim and records retry, pause, cancel, or failure without rewriting the specification |
| Terminal | Task and Subagent outcome terminals have separate owners; linkage exists only where the runtime records correlation |
| Recovery and projection | Checkpoint/claim recovery belongs to the Task owner; Todo and UI remain projections |

Plan is a reviewable artifact, not another runtime state machine. Workflow is an
adjacent orchestration capability and does not replace the revisioned Task graph.
See [Tasks](./09-tasks.md), [Subagent](./06-subagent.md),
[Multi-Agent Patterns](./26-multi-agent.md), and [Runtime](./29-long-running-tasks.md).

## Tool, Permission, And Effect

```text
automatic Agent call
  -> resolve -> React Permission pipeline -> ToolManager validation / admission
direct ToolManager call
  -> caller-owned policy boundary -> ToolManager validation / admission
both
  -> backend executes Effect -> typed result / observation -> backend cleanup
```

| Boundary | Contract |
| --- | --- |
| Trigger | Automatic React Agent Tool call or direct framework ToolManager invocation |
| Admission | ToolManager validation precedes its cache/permit/effect path; the automatic React path evaluates Permission before invoking it, while direct callers own their outer policy |
| Authority | ToolManager owns validation, dispatch, cache, and admission; PermissionService owns decisions only where the caller composes it; backend owns the Effect |
| Events and effects | Result, Trace, and audit observe file, process, network, sandbox, or MCP actions |
| Cancellation and failure | Cancellation and cleanup are backend-specific; a caller may claim settlement only where that backend contract awaits it |
| Terminal | A typed Tool result or error; observer text cannot replace it |
| Recovery and projection | Idempotency, retry, and cleanup use backend policy; Trace/audit remain observations |

Prompt, project rule, or Plan can guide behavior but cannot grant Permission.
Framework policy applies to automatic Agent effects. Interactive product policy
belongs to the embedding application. Direct ToolManager use does not implicitly
run PermissionService. Hook reduction, protected paths, read-only classification,
and layered shell policy retain their detailed contracts; this overview does not
claim they already form one universal decision owner. See [Tools](./02-tools.md),
[Human Loop](./05-human-loop.md), [Security](./security.md), and
[Guard System](./18-guard-system.md).

## Observation And Delivery

```text
domain event or fact
  -> named authority commits or publishes
  -> EventEnvelope supplies identity and order
  -> Journal / Delivery Ledger / explicit Outbox records its own domain
  -> Projection, Feed, history, and Trace consume
  -> retention / Checkpoint / generation fence
```

| Boundary | Contract |
| --- | --- |
| Trigger | Domain commit, runtime event, delivery request, or diagnostic observation |
| Admission | The named authority validates identity, ordering, and generation for its domain |
| Authority | Journal owns journal-backed facts; Delivery Ledger or Outbox owner owns delivery lifecycle; RunStore owns Trace records |
| Events and effects | EventEnvelope carries identity/order; a delivery Effect may occur outside the process |
| Cancellation and failure | Unknown delivery outcomes require reconciliation; lag or EOF is not success |
| Terminal | Domain commit and delivery settlement are explicit; Trace terminal is diagnostic |
| Recovery and projection | Replay/checkpoint rebuild projections; retention never turns a projection into fact |

There is no workspace-wide global Journal. Each domain declares whether it is
journal-backed. See [Persistence Concepts](./41-persistence-concepts.md),
[Delivery Ledger](./41-delivery-ledger.md), and [Tracing](./27-tracing.md).
Trace, Metrics, and Telemetry diagnose execution; their exporters and retention
do not become business commit authorities.

## Extension

```text
discover -> parse -> prepare -> validate -> component publication
       -> activate / use
replace / reload -> component-specific old/new generation coordination
withdraw request -> component-specific close / cleanup observation
```

| Boundary | Contract |
| --- | --- |
| Trigger | Project, user, Plugin, SDK, or application configuration |
| Admission | Parse and validate identity, scope, capabilities, and required resources before publication |
| Authority | MCP, Hook, Skill, Plugin, and LSP retain their own registries and resource owners; no universal active generation owns all of them |
| Events and effects | Tool/resource registration, child process, network connection, resource transfer, Hook action, or Skill activation |
| Cancellation and failure | Partial publication, rollback, and cleanup debt are component-specific and must remain observable; current components do not share one settlement protocol |
| Terminal | A close is terminal only when the specific owner reports its resources settled; not every management API awaits every child resource |
| Recovery and projection | Where generation fencing exists it is component-owned; stale derived-handle coverage, catalogs, and status views follow the detailed component contract |

Plugin publication can compose components without erasing child cleanup
responsibility. The framework does not currently claim one production
coordinator, universal generation fence, or awaited close across every extension.
Current behavior must be read from [MCP](./08-mcp.md),
[Hooks](./23-hooks.md), [Skills](./07-skills.md), [Plugins](./32-plugin-system.md),
and [LSP](./31-lsp-integration.md); this overview does not promise atomic hot reload.

## SDK Consumer

The multilingual SDK and its source-built ACP Host are maintained in the
independent [echo-agent-sdk](https://github.com/EchoYue-lp/echo-agent-sdk)
repository. It consumes this framework's Rust facade and runtime authorities;
the framework does not own the SDK's language contracts, generated catalogs, or
client lifecycle implementations.

## Failure And Terminal Matrix

| Signal | What it means | What it does not mean |
| --- | --- | --- |
| Accepted | The owner admitted work and established identity | The work succeeded or all Effects started |
| Cancellation requested | The owner should stop according to policy | The producer, child process, or cleanup has settled |
| Timeout observed | A caller's budget expired | The underlying work is terminal unless its owner publishes that result |
| Stream EOF | No more frames arrived on that stream | Successful Turn, Tool, delivery, or protocol terminal |
| Trace terminal | A diagnostic Run reached a recorded status | The product Task or Turn committed the same terminal |
| Completed receipt | The named lifecycle published successful terminal | Every external Effect is exactly once |
| Failed or Cancelled receipt | The named lifecycle published a non-success terminal | All other correlated lifecycles share that same status |

## Example Routes

| Lifecycle | Executable consumer |
| --- | --- |
| Agent Turn and Tool | [`demo01_tools`](../../echo-agent-learning/examples/demo01_tools.rs) |
| Context and compression | [`demo53_adaptive_compression`](../../echo-agent-learning/tests/example_contracts/demo53_adaptive_compression.rs) |
| Task and Subagent | [`demo04_subagent`](../../echo-agent-learning/tests/example_contracts/demo04_subagent.rs) |
| Workflow events | [`demo34_workflow_stream`](../../echo-agent-learning/tests/example_contracts/demo34_workflow_stream.rs) |
| MCP lifecycle | [`demo30_mcp_server`](../../echo-agent-learning/tests/example_contracts/demo30_mcp_server.rs) |
| Eval and Trace | [`demo50_eval`](../../echo-agent-learning/tests/example_contracts/demo50_eval.rs) |
| Headless projection | [`demo54_headless`](../../echo-agent-learning/tests/example_contracts/demo54_headless.rs) |

## Further Reading

- [Framework Architecture](./architecture.md)
- [Core Concepts](./concepts.md)
- [Runtime and Tasks](./29-long-running-tasks.md)
- [Persistence Concepts](./41-persistence-concepts.md)
- [Framework and Application Boundary](./39-framework-application-boundary.md)
- [ADR 0040](../adr/0040-framework-concept-documentation-authority.md)
