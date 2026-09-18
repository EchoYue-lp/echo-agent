# ADR 0059: Task Graph and Workflow Graph Authorities

## Status

Accepted on 2026-09-18. Resolves the boundary decision in Issue #111; it does
not merge three public graph APIs or change their execution contracts.

## Context

`RevisionedTaskGraph`, `Graph`, and `DagWorkflow` all describe nodes and edges,
but their edges carry different meaning. Without an explicit owner for each
graph, a new dependency feature could copy Task claim/retry rules into a
Workflow, or treat a static pipeline result as a committed Task terminal.
Conversely, deleting a public Workflow because EKO does not invoke it would
remove a reasonable framework capability. ADR 0040 defines Workflow checkpoint
leases and sibling settlement; ADR 0052 gives `Graph` one entry loop. Neither
decides the boundary between Task dependencies and the two Workflow APIs.

The existing implementations and their real entry points are:

| Contract | Authority | Edges and execution | State and recovery |
| --- | --- | --- | --- |
| Revisioned Task graph | `TaskRevisionService` for graph revisions and validation; `RuntimeTaskService` for ready frontier and dispatch | Mutable `TaskSpec.depends_on` relationships, atomically patched through `task_create`/`task_update`/`task_list`; a committed revision drives Subagent claims and bounded waves | `RevisionedTaskStore` holds specs/executions; exact claim/attempt CAS, retry, pause and cancellation settlement are Task facts |
| `Graph` | `GraphBuilder` for topology; `Graph::execute_loop` for node transitions | Caller-defined conditional branches, cycles, fan-out and finish nodes over `SharedState`; YAML/JSON loader and `StateGraph` lower to `GraphBuilder` | `CheckpointStore` owns a Workflow continuation and its lease/generation; `WorkflowEvent` observes the same loop |
| `DagWorkflow` | `DagWorkflowBuilder` for a fixed acyclic pipeline; `DagWorkflow::run` for batch scheduling | Agent nodes consume predecessor text outputs; independent nodes execute concurrently in topological batches | `WorkflowOutput` records this invocation's steps/result; it has no Task revision, durable claim or checkpoint/resume contract |

## Industry Evidence

- [Temporal Workflow Execution](https://docs.temporal.io/workflow-execution)
  treats workflow history as the recovery authority for workflow state, while
  [Activities](https://docs.temporal.io/activities) are separately retried
  execution units. This supports distinguishing a persisted graph continuation
  from an independently claimed unit of work.
- [LangGraph persistence](https://docs.langchain.com/oss/python/langgraph/persistence)
  binds graph checkpoints to a thread; [LangGraph Pregel execution](https://github.com/langchain-ai/langgraph/blob/230927fb3a9ac9b2893a30322b4dfea7cdea9a8f/libs/langgraph/langgraph/pregel/main.py#L3783-L4095)
  gives graph invocation and streaming one execution path. ADR 0052 already
  applies that pattern within `Graph`, not across Task and Workflow contracts.
- [Tokio graceful shutdown](https://tokio.rs/tokio/topics/shutdown) separates
  a cancellation signal from waiting for task completion. This is relevant to
  both authorities, but sharing that lifecycle principle does not make their
  persisted facts interchangeable.

The sources illustrate ownership and lifecycle patterns, not an assertion that
these systems expose the same three Rust APIs. The implementation and tests in
this repository determine their exact contracts.

## Options Considered

1. Make the revisioned Task graph the only graph engine. Rejected: Workflow
   conditional routing, cycles, `SharedState`, interruption and checkpoint
   continuation are not Task dependency or claim semantics; replacing the
   public Workflow API would break valid framework consumers.
2. Make `Graph` or `DagWorkflow` execute every Task graph. Rejected: their
   process-local node result or Workflow checkpoint cannot atomically commit
   Task revisions, exact claims, retry and pause receipts. A wrapper retaining
   those decisions would recreate the Task scheduler beside the Workflow one.
3. Keep three independent contracts with explicit authority and thin
   composition. Chosen. Pure, side-effect-free graph algorithms may be shared
   later when the inputs and failure behavior really match; no shared
   scheduler, store or state machine is justified by graph-shaped data alone.

## Decision

The revisioned Task graph is the only authority for dynamic PlanTask
relationships and their execution status. A `TaskPlan` is a versioned,
reviewable artifact and a Todo list is a projection; neither owns a scheduler.
`RuntimeDagController` may load/persist committed Task facts, dispatch a
Subagent and apply product policy, but may not duplicate the ready frontier,
dependency validation, retry or terminal reduction.

`Graph` is the authority for its own caller-authored routing and Workflow
continuation. `DagWorkflow` remains a separate static, acyclic text pipeline
under the public `Workflow` trait. It does not acquire `Graph` checkpoint
semantics merely because both live in the Workflow module. A Workflow invocation
may run as a Task's dispatched operation, but its node events and output are
operation evidence. Only the exact Task claim settlement API can commit that
Task's terminal status. Workflow and Task identifiers, revisions and
checkpoints must not be silently converted into one another.

An application can select a Workflow for a Task and retain a product-owned
association of identities. Its adapter passes input and returns a typed
success/failure/cancellation outcome to the Task controller; the controller
settles the original claim. If cancellation or an external effect has not
settled, the adapter must report that uncertainty and must not infer Task
success from `WorkflowEvent::Completed`, stream EOF or `WorkflowOutput` alone.
There is no automatic conversion from a Task graph to `DagWorkflow` or `Graph`.
Any future explicit converter must preserve dependency, revision and attempt
identity, prove field-level round trips, and keep the Task owner responsible
for claims and terminal decisions.

## Consequences

Framework consumers retain all three public options with clear use cases.
Graph-specific fixes continue to use `Graph::execute_loop` and
`CheckpointStore`; static pipeline fixes use `DagWorkflowBuilder` and
`DagWorkflow::run`; Task scheduling fixes use `TaskRevisionService` and
`RuntimeTaskService`. Shared cancellation or graph validation helpers can be
extracted only after equivalent semantics are demonstrated with tests. This
decision does not close any separate Workflow validation or Task recovery bug.

This ADR changes documentation and a documentation contract only; it adds no
serialization, state, runtime adapter, SDK mapping or migration. Reverting it
restores the former undocumented boundary without changing runtime behavior.
Its long-term assertion is guarded by the learning documentation contract,
while existing Task and Workflow tests verify their execution paths.

## References

- Issue #111 and `finding.workflow-dag-authority`.
- `echo-orchestration/src/tasks/revisioned.rs`, `runtime_service.rs` and
  `runtime_executor.rs`.
- `echo-orchestration/src/workflow/graph.rs`, `dag.rs` and
  `checkpoint_store.rs`.
- [ADR 0040: Workflow checkpoint lease](0040-workflow-checkpoint-lease-and-sibling-settlement.md).
- [ADR 0052: Workflow entry loop](0052-workflow-entry-loop-authority.md).
