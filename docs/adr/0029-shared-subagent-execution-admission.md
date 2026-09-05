# ADR 0029: Shared Subagent Execution Admission

- Status: Proposed
- Date: 2026-09-04
- Scope: `echo-agent::agent::admission`, `echo-agent::agent::subagent`,
  `echo-agent::echo-orchestration::tasks`

## Context

The framework already owns the reusable execution mechanisms needed by an Agent application:

- `RuntimeTaskService` drives a revisioned task graph and applies a local per-runtime semaphore
  for parallel task dispatch;
- `NestedDelegationPolicy` carries delegation depth and whether a caller may spawn a child;
- `SubagentExecutor` routes `Sync`, `Fork`, `Teammate`, and `Team` through one dispatch engine;
- `KeyedExecutionAdmission` owns opaque-key leases, optional process permits, retirement fences,
  close, and shutdown waiting.

An application can nevertheless bypass a common Subagent limit when it combines these mechanisms:
the task runtime may use one semaphore, the Subagent executor may use a Fork-only semaphore, and
the application may add a third process governor. `Sync` can then bypass a Fork limit, and direct
or nested `agent_tool` dispatch can bypass a TaskRuntime limit entirely.

This is a generic execution-lifecycle problem. The framework must provide the shared admission
composition point; the application must retain the meaning of its product capacity, key scope,
and defaults.

## Existing framework evidence

- `echo-agent/echo-orchestration/src/tasks/runtime_executor.rs` already owns task graph revision
  safe points, ready waves, local concurrency, cancellation, and terminal settlement.
- `echo-agent/echo-core/src/tools/mod.rs` already owns `NestedDelegationPolicy` and its child
  policy transition.
- `echo-agent/src/agent/subagent/executor.rs` already provides one dispatch engine for all
  Subagent execution modes and currently keeps `max_concurrent_forks` as a Fork-local semaphore.
- `echo-agent/src/agent/admission.rs` already provides the lease/permit lifecycle that EKO's
  `AgentPoolAdmission` wraps.

## Decision

1. Extend the existing framework admission boundary so a caller can provide one shared execution
   admission to both `RuntimeTaskService` and `SubagentExecutor`. Do not create an EKO-specific
   `SubagentAdmission` state machine.
2. A shared admission lease represents one active execution identity, normally a unique
   `SubagentRun`/attempt key. Its process semaphore and lease lifecycle are owned by framework
   primitives; the application supplies the capacity and key namespace.
3. When a shared admission is configured, every Subagent execution mode that creates a
   `SubagentRun` (`Sync`, `Fork`, `Teammate`, and concrete Team members) obtains the same shared
   lease. `max_concurrent_forks` is not a bypass path in this mode.
4. `RuntimeTaskServiceConfig.max_concurrent_subagents` remains available to standalone framework
   consumers as a per-runtime scheduling width. It is not automatically the process-wide shared
   capacity. A caller may derive the local width from the shared capacity, but the two authorities
   must not maintain independent product counters.
5. `SubagentExecutorConfig.max_concurrent_forks` remains a standalone compatibility fallback for
   consumers that do not configure shared admission. EKO must not expose it as a second product
   setting once shared admission is wired.
6. `NestedDelegationPolicy` remains the sole depth policy. Framework consumers choose their own
   maximum; EKO supplies `max_delegate_depth = 1` and `can_spawn_subagents = true` only for the
   primary context. Child contexts receive the existing child policy and cannot delegate again.
7. `KeyedExecutionAdmission` instances for AgentPool leases and SubagentRun leases remain
   separate resource classes even though they use the same framework primitive. Agent instance
   reuse, workspace retirement, and foreground execution must not be counted as SubagentRun
   capacity.

## Alternatives considered

1. **Keep EKO's process Subagent governor and add more adapters.** Rejected because direct,
   nested, Sync, and Fork paths would continue to have multiple authorities.
2. **Delete framework local concurrency settings.** Rejected because standalone framework
   consumers still need a per-runtime scheduling width and Fork fallback.
3. **Put EKO's default, workspace, or UI policy in the framework.** Rejected because those are
   application decisions and would pollute a reusable crate.
4. **Extend the existing admission primitive and inject it into both framework runtimes.**
   Accepted because it preserves framework ownership of lease/permit lifecycle while keeping
   product capacity and key policy with the application.

## Consequences

- Framework gains one generic integration point for shared execution capacity without knowing
  EKO or Subagent product policy.
- EKO can expose one `max_concurrent_subagents` value and remove its duplicate Subagent process
  governor and raw TaskRuntime/Executor bridging.
- Standalone framework consumers retain existing local configuration and are not forced into a
  process-global policy.
- Tests must cover shared admission across TaskRuntime/direct dispatch, all execution modes,
  cancellation, terminal release, close, and capacity rejection.
- The framework default delegation depth remains a generic default; EKO's one-level rule belongs
  in the application configuration and capability projection.

## References

## Implemented framework contract

`echo_core::agent::ExecutionAdmission` composes keyed lease lifecycle with
optional process capacity. Runtime services may inject it while retaining
per-runtime wave width; `max_concurrent_forks` remains standalone fallback.
EKO supplies product capacity and policy through its adapter.

- `echo-agent/src/agent/admission.rs`
- `echo-agent/src/agent/subagent/executor.rs`
- `echo-agent/echo-core/src/tools/mod.rs`
- `echo-agent/echo-orchestration/src/tasks/runtime_executor.rs`
- `echo-agent/docs/adr/0015-keyed-execution-admission.md`
- `echo-agent-cli/echo-agent-app-core/src/tasks/task_runtime/executor/limits.rs`
