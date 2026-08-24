# ADR 0005: Invocation Resource Lifetime Guards

- Status: Accepted
- Date: 2026-08-25

## Context

An Agent invocation can start work that outlives the future currently observed
by its caller. The ReAct loop, streaming driver, Subagent executor, and tools
also cross `tokio::spawn` boundaries. Cancellation and trace metadata already
flow by value through `AgentInvocationContext`, but neither field owns an
application resource such as a concurrency permit, temporary workspace, or
external lease.

Releasing such a resource when the caller or event stream is dropped can admit
conflicting work while an already-started tool or blocking operation is still
running. The framework must preserve ownership without learning EKO-specific
pool, workspace, or scheduling policy.

## Considered Options

1. Keep resources in application task-local state. Tokio tasks created with
   `spawn` do not inherit task-local ownership, so Subagent and tool boundaries
   lose the resource.
2. Add typed application permits to framework contexts. This couples the
   reusable framework to one product's resource model.
3. Add an opaque cloneable ownership token to the existing invocation context
   chain. Shared ownership follows the spawned work while the wrapped value
   remains inaccessible.

## Decision

Adopt option 3. `InvocationResourceGuard` stores the value behind
`Arc<dyn Any + Send + Sync>` but exposes no getter or downcast operation. Its
public surface supports construction, cloning, dropping, and redacted Debug
only. The read-only `retains::<T>()` predicate checks the exact wrapped type so
consumers can select their own guard among several resources, but exposes no
reference, owned handle, or downcast operation. A free-form marker key was
rejected because it introduces collision, spoofing, and naming-governance
problems that the Rust type already solves.

`resource_guards` propagate through:

```text
AgentInvocationContext
  -> AgentRunSnapshot
  -> ToolContext
  -> ExternalRunContext for Subagent dispatch
  -> child AgentInvocationContext
```

Default value-scoped methods on the public `Agent` trait wrap a third-party
Agent's returned event stream with the complete invocation value. Implementors
that have not overridden these methods therefore retain guards until that
stream ends or is dropped instead of releasing them when the factory future
returns.

Invocation and runtime guard lists are both retained. Tools that create owned
asynchronous or blocking work clone the guard list into that work. Destruction
of the wrapped resource therefore occurs only after the final clone settles.
Run and turn identifiers are optional correlation metadata: a non-empty guard
list creates and propagates `ExternalRunContext` even when both identifiers are
absent, including detached background Subagent dispatch.

Legacy mutable Agent setters are captured as one value before an invocation
waits for the execution mutex. Queued calls therefore retain the run,
cancellation, trace, delegation, and guard values that belonged to their
caller. A private context epoch serializes `set`, `clear`, and every composite
capture so conversation, run, turn, execution, isolation, message,
cancellation, trace, delegation, and guard fields cannot come from different
callers. Every non-empty combination is a valid context; identifiers are not a
gate for cancellation, trace, delegation, or ownership. Replacing or clearing
the shared context swaps all fields inside the epoch and drops the previous
snapshot only after unlocking, because an application-owned destructor may
block or re-enter framework code.

This follows Rust's `Arc` shared-ownership contract and Tokio's owned-value
spawn model. It is also compatible with owned permits such as Tokio's
`OwnedSemaphorePermit`, without making the framework understand semaphore
policy.

## Consequences

- Stream and caller cancellation no longer imply premature release of a
  correctly retained application resource.
- Main Agent, Subagent, streaming, and non-streaming tool paths share one
  ownership channel rather than parallel product-specific mechanisms.
- Anonymous and background work retains the same ownership guarantees; runtime
  identity is not an ownership precondition.
- Slow or re-entrant application destructors do not run while the framework's
  context epoch or guard-container mutex is held.
- Tools remain responsible for cloning guards into work they detach from their
  own future. The framework cannot retain a resource for work it does not own
  or observe.
- Debug output includes Rust type names and guard counts, but never formats the
  wrapped values.
- The new public struct fields are intentionally source-breaking for explicit
  context literals; the project is pre-release and updates all workspace
  consumers together.

## References

- Rust `Arc`: <https://doc.rust-lang.org/std/sync/struct.Arc.html>
- Tokio `spawn`: <https://docs.rs/tokio/latest/tokio/task/fn.spawn.html>
- Tokio `OwnedSemaphorePermit`:
  <https://docs.rs/tokio/latest/tokio/sync/struct.OwnedSemaphorePermit.html>
