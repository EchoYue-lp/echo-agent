# ADR 0004: Async File Store Ownership

- Status: Accepted
- Date: 2026-08-25
- Owners: `echo-core/utils`, `echo-state/memory`, `echo-agent/state`

## Context

`RuntimeStateStore` and `ConversationStore` are asynchronous framework traits,
but their file-backed implementations previously executed `std::fs` work from
the polling task. `FileConversationStore` moved each call to
`spawn_blocking`, but every caller created an unbounded independent operation.
Dropping the caller could also drop the only observable owner before a durable
write had settled. A store-wide synchronous mutex serialized unrelated
conversations and made the ownership boundary difficult to verify.

The framework must keep file durability and error behavior unchanged: atomic
temporary-file writes, data and directory synchronization, generation manifests,
UTF-8 parsing errors, corrupt-record failures, and cross-process lease rejection
remain authoritative. EKO policy and UI state do not belong in this mechanism.

## Options Considered

1. Keep synchronous file calls inside async trait futures. This is simple, but a
   slow filesystem blocks unrelated Tokio tasks.
2. Call `spawn_blocking` independently in every method. Tokio documents that
   started blocking tasks cannot be aborted and that its default blocking limit
   is large, so this has no process resource bound or shared ordering authority.
3. Admit owned operations through one process-wide bounded service, then order
   operations by canonical store path and conversation key. This keeps the
   trait API stable while making ownership and concurrency explicit.

## Decision

File-backed async traits submit owned closures through
`echo_core::utils::blocking::run_keyed_file_operation`.

- Process admission is bounded at 64 unsettled operations and blocking execution
  is bounded at 8 concurrent closures. Tokio semaphore admission is fair.
- Admission happens before the detached owner is created. After admission, a
  process-lifetime Tokio runtime owns the admission/execution permits, key
  completion receipt, closure, and result sender until settlement. Dropping the
  calling future or shutting down its Tokio runtime cannot cancel an accepted
  durable operation or release the next same-key operation early.
- The helper uses `Handle::try_current` and returns a typed error outside a Tokio
  runtime rather than invoking an API that can panic.
- Keys hash namespace, canonical root path, and typed entity/collection scope as
  separate fields; delimiters and a real conversation named `__list__` cannot
  alias a collection scan. Entity scope and persisted filename are both derived
  from the exact UTF-8 bytes encoded as lowercase hexadecimal ASCII. Case folding
  and Unicode normalization on APFS or another filesystem cannot make distinct
  logical IDs share a file while using different queues. Each key has a checked
  monotonic generation and a shared completion tail.
  Generation exhaustion fails closed; same-key operations execute in submission
  order without holding a synchronous mutex across an `.await`.
- `FileRuntimeStateStore` keys operations by canonical base plus
  `conversation_id`.
- `FileConversationStore` shares metadata/cache/file authority between handles
  on the same canonical base. Per-conversation operations use the same keyed
  queue. Brief synchronous metadata and cache locks are acquired only inside a
  blocking closure. A scan barrier gives collection scans a stable
  manifest/message-generation snapshot while allowing unrelated conversation
  operations to overlap.
- SQLite implementations remain independent optional framework backends and are
  unchanged.

The Tokio contracts used by this decision are documented in
[`spawn_blocking`](https://docs.rs/tokio/1.53.1/tokio/task/fn.spawn_blocking.html),
[`Handle::try_current`](https://docs.rs/tokio/1.53.1/tokio/runtime/struct.Handle.html#method.try_current),
and [`Semaphore`](https://docs.rs/tokio/1.53.1/tokio/sync/struct.Semaphore.html).

## Consequences

File-backed runtime and conversation operations no longer block a Tokio runtime
thread. Same-conversation updates retain atomic ordering. Independent
conversations can progress concurrently, but a process cannot create an
unbounded number of blocking filesystem operations.

Once accepted, a write may finish after its caller is cancelled. This is
intentional: durable file mutation is non-abortable, and later same-key reads
wait for that mutation's settlement. Callers still receive the original
serialization, I/O, corruption, and missing-record semantics when they remain
attached.

## Verification

Regression tests cover a current-thread runtime heartbeat, operation completion
after caller abort, caller-runtime shutdown while a same-key operation is
parked, owner panic settlement, same-key ordering, structured-key collision
resistance, bounded different-key concurrency, typed no-runtime and
generation-exhaustion failures, save/clear/save ABA ordering, corrupt checkpoint
read followed by clear, corrupt conversation read racing delete, and a search
holding an old manifest generation while replacement waits. Concurrent
case-distinct and Unicode-normalization-distinct IDs prove that logical entities
do not alias even on case-insensitive filesystems. Existing append, replacement,
fsync boundary, Unicode, restart, lease, and recovery tests remain the durability
conformance suite.
