---
schema_version: 4
slug: 2026-09-18-task-claim-subagent-attempt-link/framework-external-attempt-control
outcome:
  summary: Expose a scope-bound SubagentAttemptControlHandle so external
    RuntimeDagController adapters can reuse the canonical exact-attempt
    lifecycle without raw registry access or durable-state ownership.
  acceptance:
    - An external Rust consumer can construct one validated scope-bound handle
      from SubagentExecutor and drive reserve, claim-derived dispatch, typed
      live interrupt projection, post-CAS retirement, and recovery
      reconciliation without crate-private access.
    - TaskClaim and RuntimeTaskService remain the only durable precondition and
      terminal authorities; raw registry mutation helpers stay private and Team
      plus external adapters share one handle implementation.
    - Focused tests, public facade compilation, applicable feature checks, the
      complete framework merge gate, semantic verification, independent review,
      remote CI, and squash delivery to main all pass.
out_of_scope:
  - Changing echo-agent-sdk framework pins, wire DTOs, language SDKs, Task graph
    persistence, command journals, or replay; those are dependent outcomes in
    the delivery map.
  - Defining SDK pause/resume product policy, EKO UI/TUI projections, or
    application storage.
  - A2A behavior or contracts.
design_ref: docs/supreme/specs/2026-09-18-task-claim-subagent-attempt-link/design.md
design_sections:
  - ref: design.md § External Task Adapter Control Contract
    digest: sha256:67e6925df9986257ede03446a8e1e0ddcfb05fbb9ff01c4830768a105183c2d1
  - ref: design.md § Live Interrupt Admission
    digest: sha256:7982383042b576e20347fc5ee1a0cc919e9c0f880f71521d0b2096c3bfa68d9c
  - ref: design.md § 异常和边界场景
    digest: sha256:c80b8e55adb0456755f94f1c6b57a73666817e3be41f2596f33fca1ee8e7a4db
  - ref: design.md § 复用与实现约束
    digest: sha256:e8d4352ec314505036e3701de8ba2ef9668cee2a6c609269734d93dfbe99e1cd
  - ref: design.md § 验收标准
    digest: sha256:b064ccfa9e71cfed98f58241b718058d1b342d27d40daf2fcb5508063eb83148
delivery_ref: docs/supreme/specs/2026-09-18-task-claim-subagent-attempt-link/plans/delivery-map.md#framework-external-attempt-control
todos:
  - id: scope-bound-attempt-control-handle
    summary: Add a public SubagentAttemptControlHandle created by SubagentExecutor
      that validates and fixes one control scope, derives every identity from
      TaskSubagentContext or TaskClaim, and delegates the full live lifecycle to
      the existing registry.
    files:
      - src/agent/subagent/control.rs
      - src/agent/subagent/executor.rs
      - src/agent/subagent/mod.rs
    acceptance:
      - Blank scopes and conflicting context/claim identity are rejected before
        registry mutation; reserve and dispatch consume one exact binding, and
        typed interrupt/retire/reconcile results preserve the fixed scope.
      - The handle does not load a Task store, judge claim currency, write Task
        status, or expose SubagentControlRegistry.
  - id: converge-team-and-external-control-path
    summary: Route the existing Team exact-control adapter through the same
      scope-bound handle and remove duplicated identity assembly or direct
      private helper calls from canonical Team execution.
    files:
      - src/agent/subagent/executor.rs
      - src/agent/subagent/team/mod.rs
    acceptance:
      - Default Team, React Team, and external adapter handle share reserve,
        dispatch, interrupt projection, retirement, and reconciliation semantics
        without a second registry or execution identity algorithm.
      - Existing sibling cancellation, delayed reservation, same-run
        multi-handle, recovery, and targeted abort regressions remain green.
  - id: public-contract-docs-and-evidence
    summary: "Document and verify the external adapter boundary, expose the handle
      through the public facade, and update #99 semantic evidence while keeping
      the SDK and Issue open."
    files:
      - tests/facade_smoke.rs
      - docs/adr/0058-task-claim-subagent-attempt-control.md
      - docs/en/29-long-running-tasks.md
      - docs/zh/29-long-running-tasks.md
      - .echo-semantic/findings/finding.task-subagent-attempt-link.md
      - .echo-semantic/evidence/
      - .echo-semantic/audits/
    acceptance:
      - A public external-consumer test compiles the handle and its typed method
        surface without accessing crate-private APIs, and docs state that live
        projection cannot replace RuntimeTaskService durable checks.
      - "Semantic evidence and independent rereview bind the delivered framework
        SHA and explicitly retain SDK pin, durable Task store, command journal,
        crash replay, and cross-repository E2E as open #99 work."
artifact_id: plan:fbd8a92d-0fb2-4f7d-803a-3e4ae82ef0d8
lifecycle: completed
---
