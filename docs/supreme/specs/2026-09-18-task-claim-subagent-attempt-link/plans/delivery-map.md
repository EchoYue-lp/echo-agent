---
schema_version: 1
artifact: delivery-map
design_ref: docs/supreme/specs/2026-09-18-task-claim-subagent-attempt-link/design.md
outcomes:
  framework-exact-attempt-control:
    ships: Deliver the framework phase of TaskClaim to SubagentAttempt identity,
      task-scoped cancellation, pre-admission exact interrupt, and recoverable
      live-control reconciliation without closing the cross-repository SDK
      Finding.
    depends_on: []
  framework-external-attempt-control:
    ships: Expose a scope-bound SubagentAttemptControlHandle so external
      RuntimeDagController adapters can reuse the canonical exact-attempt
      lifecycle without raw registry access or durable-state ownership.
    depends_on:
      - framework-exact-attempt-control
  sdk-exact-attempt-adapter:
    ships: Pin echo-agent-sdk to the framework exact-control revision and route Task
      execution and same-process exact control through one RuntimeTaskService
      and one scope-bound attempt handle.
    depends_on:
      - framework-external-attempt-control
  sdk-durable-task-command-replay:
    ships: "Persist the SDK Host Task graph and idempotent task-control command
      journal, recover stale process claims, replay unsettled commands, and
      close Issue #99 with cross-process E2E evidence."
    depends_on:
      - sdk-exact-attempt-adapter
design_revision: sha256:d76ae0b5451be9025685c74e84baa9568ed539e41b848d897486c2c2a8960e00
---
