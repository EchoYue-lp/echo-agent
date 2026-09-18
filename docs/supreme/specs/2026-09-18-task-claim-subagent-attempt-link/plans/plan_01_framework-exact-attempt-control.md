---
schema_version: 4
slug: 2026-09-18-task-claim-subagent-attempt-link/plan
outcome:
  summary: Deliver the framework phase of TaskClaim to SubagentAttempt identity,
    task-scoped cancellation, pre-admission exact interrupt, and recoverable
    live-control reconciliation without closing the cross-repository SDK
    Finding.
  acceptance:
    - Every Team/runtime Subagent dispatch preserves run_id, task_id,
      execution_id, plan_revision, and attempt from one TaskClaim through
      events, control, and terminal settlement.
    - Exact interrupt cancels only its task child; pre-admission reservation and
      non-cooperative target paths are bounded, typed, and sibling-safe.
    - Framework focused tests, 17 independent feature checks, complete merge
      gate, strict semantic verification, independent implementation review, and
      remote CI all pass.
out_of_scope:
  - echo-agent-sdk Host adapter, framework pin, inventory regeneration, and SDK
    E2E; those follow after the framework SHA is merged.
  - A2A changes.
  - Product-specific EKO policy, UI projections, worktrees, approvals, or a
    second task/subagent store.
design_ref: docs/supreme/specs/2026-09-18-task-claim-subagent-attempt-link/design.md
design_sections:
  - ref: design.md § 目标行为
    digest: sha256:bc012656481dfd4a35d900e5dd42f7424bdd40ccf7d0d56001e1d455ccb952d8
  - ref: design.md § 系统边界
    digest: sha256:a0f82f297bb440b8990475ca599f935743098ad05dd477239f21a74856a35840
  - ref: design.md § 核心结构与数据流
    digest: sha256:ea01984fa8a96ab3ae4f26106cfe9de8c8b3c5bc8c6c3e5cd9360b80587e6f0d
  - ref: design.md § Live Interrupt Admission
    digest: sha256:7982383042b576e20347fc5ee1a0cc919e9c0f880f71521d0b2096c3bfa68d9c
  - ref: design.md § Cancellation Tree
    digest: sha256:5be7e1e71707aa9301aeeadb4438ef426b56572396ffa2360549bb1506c60f0d
  - ref: design.md § Exact Attempt Supervisor
    digest: sha256:54edf60bb2e77ff5dfc246643447db6676d23994f2473b463dfced6812b415a7
  - ref: design.md § 异常和边界场景
    digest: sha256:728be4be29dca2b78d919d748087e9c39e071c7eac4314340be92753fc87ca38
  - ref: design.md § 验收标准
    digest: sha256:4c5586d55ce31464780b206097f047340c49928fa6a6bf5edcc69b101fb1cc32
delivery_ref: docs/supreme/specs/2026-09-18-task-claim-subagent-attempt-link/plans/delivery-map.md#framework-exact-attempt-control
todos:
  - id: exact-context-and-team-contract
    summary: Make TaskClaim-derived exact context and structured
      TeamDispatchController the only framework-to-Subagent dispatch path,
      preserving complete runtime/event lineage and a shared TeamRuntimeHandle.
    files:
      - echo-orchestration/src/tasks/runtime.rs
      - echo-orchestration/src/tasks/runtime_executor.rs
      - src/agent/subagent/team/mod.rs
      - src/agent/subagent/executor.rs
      - src/agent/subagent/events.rs
      - docs/adr/0008-canonical-runtime-task-authority.md
    acceptance:
      - Existing Team/default/React/custom dispatch callers compile only through
        the structured request and no longer generate team-member random
        identity.
      - Context construction rejects conflicting or unbound identity and
        round-trips claim lineage into event/runtime context.
  - id: reservation-and-cancellation-control
    summary: Add reservation-before-admission control, exact typed interrupt
      receipts, child-token cancellation, pending retirement, and targeted
      JoinSet supervision for non-cooperative attempts.
    files:
      - src/agent/subagent/control.rs
      - src/agent/subagent/executor.rs
      - echo-orchestration/src/tasks/runtime_executor.rs
      - echo-orchestration/src/tasks/runtime_service.rs
    acceptance:
      - Interrupt before reservation, while waiting admission, active interrupt,
        duplicate/stale identity, reclaim, and run cancellation have
        deterministic typed outcomes.
      - A non-cooperative target is bounded by the exact grace/abort path, joins
        through the canonical JoinSet, settles Cancelled when its child was
        cancelled, and does not cancel a sibling.
      - Post-CAS live cleanup failure is reported independently without
        reversing terminal state or issuing a second CAS.
  - id: runtime-handle-and-recovery-tests
    summary: Wire the execution-time TeamRuntimeHandle and durable-claim
      reconciliation/replay boundary, then add regression coverage for lost CAS
      responses, stale tombstone/restart, and active supersede drain.
    files:
      - src/agent/subagent/team/mod.rs
      - echo-orchestration/src/tasks/runtime_service.rs
      - echo-orchestration/src/tasks/runtime_executor.rs
      - src/agent/subagent/control.rs
      - tests
      - docs/en/29-long-running-tasks.md
      - docs/zh/29-long-running-tasks.md
    acceptance:
      - Default Team, React Team, and caller-supplied runtime use the same task
        store/service/control authority during execution.
      - Lost CAS response followed by reload/recovery/replay retires only proven
        non-current live projection; unknown lookup keeps projection and exposes
        retryable cleanup.
      - Focused tests cover identity, sibling isolation,
        pending/reserved/active/settled lifecycle, exact grace/join race, and no
        post-join local model/tool effects.
  - id: semantic-and-delivery-evidence
    summary: "Update framework ADR/evidence and verify the framework-only delivery
      boundary without editing SDK worktrees or closing Issue #99."
    files:
      - docs/adr/0008-canonical-runtime-task-authority.md
      - .echo-semantic/findings/finding.task-subagent-attempt-link.md
      - .echo-semantic/evidence/evidence.task-subagent-attempt-link-repair.md
      - .echo-semantic/evidence/evidence.task-subagent-attempt-link-verification.md
      - .echo-semantic/audits/audit.task-subagent-attempt-link-rereview.md
    acceptance:
      - Semantic repair, verification, and rereview evidence bind the merged
        framework SHA and explicitly record SDK as pending.
      - "Issue #99 remains open until the independent SDK phase is delivered and
        reverified."
artifact_id: plan:7d0aeb48-0dd2-4918-b082-b46884f463c5
lifecycle: completed
---
