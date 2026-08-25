# Store, Journal, Checkpoint, and Trace

These concepts are not peers at one abstraction level. A `Store` is a read/write boundary for a data domain, while `Journal`, `Checkpoint`, and `Trace` describe different semantic roles played by persisted data.

## Core distinction

| Concept      | Question answered                                           | Typical shape                 | Primary purpose                    |
| ------------ | ----------------------------------------------------------- | ----------------------------- | ---------------------------------- |
| `Store`      | Where is a kind of data kept and how is it accessed?        | trait, file or memory backend | Persistence and queries            |
| `Journal`    | What happened, in deterministic order?                      | append-only event stream      | Replay, recovery, projections      |
| `Checkpoint` | What was the state at a stable boundary?                    | state + applied sequence      | Fast recovery without full replay  |
| `Trace`      | How did an execution behave and why did it succeed or fail? | `Run` + `RunEvent`            | Debugging, diagnostics, evaluation |

```text
domain event
   |
   +-> Journal -> reduce/fold -> current state
   |                                |
   |                                +-> Checkpoint
   |
   +-> Trace

Different stores persist these artifacts or other domain data.
```

## Store: a domain persistence boundary

The framework does not define one universal Store trait for every persisted artifact. A type name ending in `Store` only identifies a domain read/write boundary; it does not determine the data model or authority.

| Interface            | Owned data                            | Scope                     |
| -------------------- | ------------------------------------- | ------------------------- |
| `Store`              | namespaced key/value long-term memory | Cross-session knowledge   |
| `ConversationStore`  | user-visible transcript projection    | Conversation browsing     |
| `RuntimeStateStore`  | `AgentCheckpoint`                     | ReAct session recovery    |
| `CheckpointStore<S>` | reducer state + applied sequence      | Event projection recovery |
| `RunStore`           | trace `Run`/`RunEvent`                | Execution observability   |

The long-term-memory `Store` supports namespace isolation, key/value operations, search, and deletion. It is one Store in the framework, not a parent interface for the other stores.

## Journal: ordered facts

`EventJournal<E>` persists ordered events instead of repeatedly overwriting current state. Its key invariants are:

- append-only records;
- contiguous sequences starting at 1;
- ordered suffix replay;
- commit before reducer projection;
- recovery of a torn trailing record, while mid-history corruption is an error;
- unknown batch-commit outcomes require reopen/reconciliation rather than blind retry.

A Journal is suitable for facts that have happened. Current state, lists, and UI DTOs may be projected from it, but projections must not silently replace the fact history.

## Checkpoint: state at a stable boundary

### Reducer checkpoints

`CheckpointedReducer` folds Journal events into state and uses `CheckpointStore<S>` to persist the applied sequence and the corresponding reducer state.

```text
Journal:      1 2 3 4 5 6 7 8 9 10
Checkpoint:              state@7
Recovery:                 load@7 + replay 8..10
```

`FileCheckpointStore` uses atomic replacement, a schema version, and a SHA-256 digest so partial or modified data is not accepted as a trusted replay prefix. For an event-sourced projection, this checkpoint is a rebuildable accelerator and does not replace the Journal.

### AgentCheckpoint

`AgentCheckpoint` is a different checkpoint type. It stores messages, current plan text, active skills, blocked reason, working directory, and capture time for resuming a ReAct execution through `RuntimeStateStore`.

Restoration validates assistant tool calls and tool results as paired messages. This prevents provider-invalid context and avoids replaying already completed side effects.

`AgentCheckpoint` owns only ReAct runtime state. It does not own an application task DAG and is not the user-visible transcript stored by `ConversationStore`.

`AgentInvocationContext` may separate these identities explicitly. `runtime.conversation_id`
remains the product/event/transcript identity, while `runtime_state_id` selects the
`RuntimeStateStore` checkpoint key. When they differ, callers should also set
`transcript_generation_id` to the runtime incarnation. The framework then adds a typed
generation + ordinal to canonical transcript records and persists only ordinal/digest cursor
state in `AgentCheckpoint`. Repeated safe points and checkpoint/product-store crash cuts are
idempotent even when two turns have identical content; compaction realigns the cursor only after
the complete pre-compaction transcript is durable.
One shared Agent may process multiple value-scoped invocations: a change in effective
`runtime_state_id` forces exact reset/restore before model preparation, while same-ID warm context
is reused. The runtime records `Hydrating(target)` before cancellable mutation and commits
`Hydrated(target)` only after restore hooks settle; non-exact states are rebuilt. Runtime switches
also clear rollback snapshots. Restore and save use the same precedence: explicit invocation
runtime ID, invocation product conversation, legacy external conversation, then configured
conversation. Together these rules prevent A -> B -> A from writing A messages into B.

Rotating `runtime_state_id` starts a clean model context without deleting the stable product
conversation. `save_checkpoint_for_scope` durably indexes each runtime ID under that product
scope. After its admission/settlement barrier, reset uses
`clear_persisted_runtime_incarnation` to reclaim the exact retired checkpoint and any
incarnation-keyed transcript while keeping the stable transcript. Product deletion uses
`delete_persisted_conversation` to clear the complete runtime lineage and incarnation transcripts
before deleting the stable transcript. See [ADR 0006](../adr/0006-runtime-state-scope-lineage.md).

### Other checkpoint domains

The framework also uses the name for distinct domains:

- compression checkpoints record context-compression boundaries;
- Git checkpoints are tags created before file mutations for worktree rollback;
- trace `Checkpoint`/`CheckpointResumed` events observe checkpoint activity but are not the checkpoint itself.

Always qualify which checkpoint and recovery source is being discussed.

## Trace: execution observability

A Trace represents one Agent invocation as a `Run`. It can include identities, agent/model/provider data, input/output/error/status, LLM usage and timing, tool activity, compression, phase transitions, tests, file edits, Subagent dispatches, and checkpoint events.

Traces are persisted through `RunStore` and consumed by analyzers, evals, replay, and diagnostic tools.

| Comparison                     | Journal                         | Trace                                            |
| ------------------------------ | ------------------------------- | ------------------------------------------------ |
| Goal                           | Preserve domain facts           | Explain execution behavior                       |
| Drives domain projections      | Usually                         | No                                               |
| Suitable as recovery authority | Determined by the domain design | Not by default                                   |
| Typical content                | State-transition events         | LLM, tool, usage, timing, and error observations |

If trace persistence may fail while the main execution continues, the Trace cannot also be the business commit authority.

## Design rules

Before adding or changing these components, answer:

1. Is this a new persistence backend or a new data semantic? Only the former is primarily a Store problem.
2. Which data is the non-lossy fact source? Prefer extending an existing Journal when ordered recovery is required.
3. Is a Checkpoint a derived Journal accelerator or an independent runtime snapshot? Document its authority and rebuild source.
4. May execution continue after a Trace write failure? If so, Trace cannot decide whether business work committed.
5. Does the same scope already have a Journal, Checkpoint, Store, or projection? Do not create parallel semantic owners.

## Code map

- Long-term-memory `Store`: `echo-core/src/memory/store.rs`
- `ConversationStore`: `echo-core/src/memory/conversation.rs`
- Journal and checkpointed reducer: `echo-state/src/journal/mod.rs`
- File Journal/checkpoint implementations: `echo-state/src/journal/file.rs`
- `AgentCheckpoint` / `RuntimeStateStore`: `src/state/mod.rs`
- Trace `Run` / `RunEvent` / `RunStore`: `src/trace/mod.rs`

See also: [Memory](03-memory.md), [Context Compression](04-compression.md), [Tracing](27-tracing.md), and [Git Isolation](34-git-isolation.md).
