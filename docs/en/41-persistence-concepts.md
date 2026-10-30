# Store, Journal, Checkpoint, and Trace

These concepts are not peers at one abstraction level. A `Store` is a read/write boundary for a data domain, while `Journal`, `Checkpoint`, and `Trace` describe different semantic roles played by persisted data.

## Core distinction

| Concept      | Question answered                                           | Typical shape                 | Primary purpose                    |
| ------------ | ----------------------------------------------------------- | ----------------------------- | ---------------------------------- |
| `Store`      | Where is a kind of data kept and how is it accessed?        | trait, file or memory backend | Persistence and queries            |
| `Journal`    | What happened, in deterministic order?                      | append-only event stream      | Replay, recovery, projections      |
| `Checkpoint` | What was the state at a stable boundary?                    | Journal identity + state + sequence | Fast recovery without full replay  |
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
| `CheckpointStore<S>` | Journal generation + reducer state + applied sequence | Event projection recovery |
| `RunStore`           | trace `Run`/`RunEvent`                | Execution observability   |

The long-term-memory `Store` supports namespace isolation, key/value operations, search, and deletion. It is one Store in the framework, not a parent interface for the other stores.

## Journal: ordered facts

`EventJournal<E>` persists ordered events instead of repeatedly overwriting current state. Its key invariants are:

- append-only records;
- contiguous sequences starting at 1;
- one stable `JournalIdentity` per fact-history generation;
- ordered suffix replay;
- commit before reducer projection;
- append receipts carry the committing Journal identity, including across typed adapters;
- recovery of a torn trailing record, while mid-history corruption is an error;
- unknown batch-commit outcomes require reopen/reconciliation rather than blind retry.

A Journal is suitable for facts that have happened. Current state, lists, and UI DTOs may be projected from it, but projections must not silently replace the fact history.

## Checkpoint: state at a stable boundary

### Reducer checkpoints

`CheckpointedReducer` folds Journal events into state and uses `CheckpointStore<S>` to persist the exact `JournalIdentity`, applied sequence, and corresponding reducer state. A sequence has meaning only inside its Journal generation; equality of sequence numbers does not make two Journals interchangeable.

```text
Journal:      1 2 3 4 5 6 7 8 9 10
Checkpoint:   journal=A, state@7
Recovery:     require journal=A, then load@7 + replay 8..10
```

`FileCheckpointStore` uses atomic replacement, a schema version, and a SHA-256 digest over Journal identity, sequence, and state, so partial, modified, or cross-Journal data is not accepted as a trusted replay prefix. If the identity differs while the complete Journal is retained, recovery discards the checkpoint, replays from the authoritative facts, and repairs it. If the Journal prefix has been pruned, identity mismatch fails closed because the missing facts cannot be rebuilt.

`apply_committed` validates the identity on an externally obtained append receipt before folding any record. Adapters may map payload types, but they must preserve the physical Journal identity together with batch identity and sequence.

File Journal batches, checkpoints, and segmented retention markers use identity-bearing schema version 2. Version 1 did not carry enough information to prove the binding and is therefore rejected rather than guessed. See [ADR 0055](../adr/0055-checkpoint-journal-identity.md).

### AgentCheckpoint

`AgentCheckpoint` 保存 ReAct 继续运行所需的消息、激活技能、blocked reason、working directory 和时间戳，由 `RuntimeStateStore` 持久化。它不承载计划副本；版本化任务图与计划 artifact 由 `TaskRevisionService` 持有。当前payload统一显式schema3，合法无cursor的runtime-only/raw用途仍保留；旧裸数组、缺版本与旧迁移恢复义务退出，不能据此删除通用Store或当前CAS/恢复能力。

Restoration validates assistant tool calls and tool results as paired messages. This prevents provider-invalid context and avoids replaying already completed side effects.

`AgentCheckpoint` owns only ReAct runtime state. It does not own an application task DAG and is not the user-visible transcript stored by `ConversationStore`. When transcript projection is enabled, its private versioned payload may carry one complete `PendingTranscriptProjection`; that value is durable intent until a ConversationStore receipt confirms the committed fact.

`AgentInvocationContext` may separate these identities explicitly. `runtime.conversation_id`
remains the product/event/transcript identity, while `runtime_state_id` selects the
`RuntimeStateStore` checkpoint key. When they differ, callers should also set
`transcript_generation_id` to the runtime incarnation. The framework then adds a typed
generation + ordinal to canonical transcript records and persists ordinal/digest cursor
state in `AgentCheckpoint`. Repeated safe points and checkpoint/product-store crash cuts are
idempotent even when two turns have identical content; compaction realigns the cursor only after
the complete pre-compaction transcript is durable.

Compression-derived summaries and rewritten tool payloads are model-context
occurrences, not new transcript events. Internal checkpoint payload version 3
keeps the original strictly ordered fact cursor and optional `context_only`
position/digest bindings for derived occurrences. Full messages and bindings are
saved in the same runtime CAS; pending checkpoints validate their current view
against cursor-after. A new same-text turn still gets fresh ordinals, and clearing
the active view preserves the append frontier.
当前 reader 只接纳显式 schema version 3；version 1/2、缺版本和旧裸数组明确拒绝，
不自动迁移或推断来源。合法 runtime-only/raw 保存用途继续保留。

Managed compression releases the Context lock while Store callbacks read its
coherent pre-view. Candidate observer/cache updates become accepted only after
checkpoint CAS succeeds. Failure or cancellation restores the volatile view and
requires authoritative hydration before another managed write; a timeout may
already have committed. Recovery does not replay a MemoryPromoter or claim to undo
its external effects. See [ADR 0056](../adr/0056-durable-transcript-projection-settlement.md).

The canonical root prompt is identified by `CanonicalContext.system_prompt`.
Working-directory refresh and root prompt setters preserve other System messages,
including restored summaries and Global Objectives, and retain projection scopes.
They reuse an already present desired root. A standalone ContextManager without
canonical context keeps its original first-System update behavior.

Live root rendering preserves runtime-override priority over the configured base
and the existing CoT, model-profile suffix and project-rule layers. A base prompt
update or skill injection keeps an active override; `replace_system_prompt`
explicitly clears it. The borrowed trait getter still reports the base prompt.
Refresh and reset use the same effective renderer; reset accepts its final root
and canonical metadata together while retaining its existing scope-reset behavior.

When `transcript_generation_id` is present, it must equal the effective runtime-state identity.
The framework rejects mismatches before admission side effects and checks again before a
checkpoint write, so it cannot persist a checkpoint that recovery will reject for identity drift.
One shared Agent may process multiple value-scoped invocations: a change in effective
`runtime_state_id` forces exact reset/restore before model preparation, while same-ID warm context
is reused. The runtime records `Hydrating(target)` before cancellable mutation and commits
`Hydrated(target)` only after restore hooks settle; non-exact states are rebuilt. Runtime switches
also clear rollback snapshots.
restore/save 使用同一优先级：显式 invocation runtime ID、invocation 产品 conversation、
configured conversation。旧 external setter 入口已退役；A -> B -> A 不会把 A 的消息写入 B。

Rotating `runtime_state_id` starts a clean model context without deleting the stable product
conversation. `save_checkpoint_for_scope` durably indexes each runtime ID under that product
scope. After its admission/settlement barrier, reset uses
`clear_persisted_runtime_incarnation` to settle any durable pending projection and retire the exact
runtime checkpoint while keeping transcript data.
该协调入口只接纳具备 managed 能力的 Store pair，不顺带删除同名 transcript；合法 raw CRUD
由调用方显式执行。产品删除要求调用方保留稳定 `ManagedConversationDelete` request，并调用
`delete_persisted_conversation_managed`，在 timeout、重启、回执丢失和新 epoch 重建后仍复用
同一 request identity。无稳定 identity 的旧 `delete_persisted_conversation` 入口已退役，
不能由便捷调用猜测旧删除或重建后的目标。
Managed generations use revision CAS and retirement tombstones, while product deletion persists a
complete scope manifest, the full dropped-operation set, and an epoch-fenced ConversationStore
receipt. See [ADR 0006](../adr/0006-runtime-state-scope-lineage.md) and
[ADR 0056](../adr/0056-durable-transcript-projection-settlement.md).

managed clear/delete 的 context-aware 变体贯穿同一 caller-owned absolute deadline，
便利入口也使用既有有界期限。不具备 managed／deadline 能力的 Store pair 返回 Unsupported；
不保留无界兼容清理路径，也不影响显式 raw Store CRUD。

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
3. Is a Checkpoint a derived Journal accelerator or an independent runtime snapshot? Document its authority, generation identity, and rebuild source.
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
