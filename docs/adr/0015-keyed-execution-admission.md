# ADR 0015: Keyed Execution Admission

- Status: accepted
- Date: 2026-08-30
- Scope: `echo-agent::agent::admission`

## Context

Applications often reuse one Agent or execution resource by an opaque
conversation, run, or session key. The application must retain a lease until
the real operation settles, prevent a retiring key from being reused, and wait
for all accepted work during shutdown. EKO had an application-local admission
reducer for these generic facts, mixed with EKO AgentPool creation, workspace,
plugin, and model policy.

The generic lifecycle is useful to any Agent application, but the meaning of a
key and the resource selected for that key belong to the consumer. The
framework therefore needs a small keyed admission primitive, not an EKO pool
or workspace service.

## Decision

`KeyedExecutionAdmission` is the framework authority for opaque-key execution
leases. It provides:

- `issue(key)` for an unbounded keyed lease;
- `issue_process_scoped(key, semaphore)` for one process permit per active key;
- `begin_retirement(key)` to fence new leases until the receipt is dropped;
- `close()` to reject new leases while existing work drains;
- `wait_key_idle(key)` and `wait_until_idle()` for cancellation-safe settlement;
- `active_count()`, `is_active(key)`, and `is_retiring(key)` for bounded
  observation.

The first lease for a key owns one process semaphore permit. Additional leases
for that key reuse the permit. The final lease drop decrements the keyed and
total counts, releases the permit, and wakes waiters. All counter transitions
are performed by the framework lease destructor; callers cannot manually settle
the same lease twice.

The key is opaque to the framework. Capacity classes, Agent creation, cache
eviction, workspace generation, plugin/model publication, and product error
mapping remain application policy. An application adapter may attach an
`AgentHandle` and convert framework errors, but it cannot keep a second
active/by-key/retiring state machine.

## Alternatives considered

1. **Keep the EKO reducer and expose a framework wrapper.** Rejected because
   the wrapper would leave two lifecycle authorities.
2. **Move the entire EKO AgentPool into the framework.** Rejected because
   creation, workspace, plugin, model, tool, and surface policy are product
   concerns.
3. **Use only a process semaphore.** Rejected because it cannot represent
   per-key reuse, retirement fences, or exact shutdown waiting.
4. **Extract keyed admission only.** Accepted because it is independently
   testable, product-neutral, and leaves key meaning and resource policy with
   the embedding application.

## Compatibility and safety

- The primitive has no EKO, Tauri, workspace, persistence, or UI dependency.
- Closed and retiring admission return typed errors; permit exhaustion is
  distinguishable from a closed semaphore.
- Lease and retirement receipts are non-cloneable and exact-owner checks are
  available to adapters.
- Poisoned internal mutexes are recovered without panicking; public operations
  do not use unchecked indexing or byte-level string slicing.
- Existing framework turn mailbox, Subagent control, ToolManager, Task graph,
  journal, and plugin preparation authorities remain unchanged.

## Consequences

Framework consumers can reuse keyed execution admission without importing EKO.
EKO keeps its AgentPool API and product key classification while its admission
and retirement state is backed by the framework primitive. Future consumers can
choose their own key and capacity policy without creating another lifecycle
reducer.

## References

- `echo-agent/src/agent/admission.rs` — implementation and contract tests.
- `echo-agent-cli/echo-agent-app-core/src/agent_pool/admission.rs` — EKO thin
  adapter.
- `echo-agent-cli/echo-agent-app-core/src/agent_pool/pool.rs` — EKO AgentPool
  creation and key policy.
