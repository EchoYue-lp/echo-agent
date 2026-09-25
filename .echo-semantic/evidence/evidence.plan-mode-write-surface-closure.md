---
schema_version: 1
id: evidence.plan-mode-write-surface-closure
kind: evidence
observed_at: source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74
source_refs:
  - src/agent/react/run/pipeline.rs
  - src/agent/snapshot.rs
  - echo-execution/src/tools.rs
  - echo-tools/src/files/files.rs
supports: [finding.plan-mode-write-surface, behavior.effect-permission-execution]
limitations:
  - Retry-delay timing has no separate fixture; every physical retry attempt uses the checked admission path
  - Third-party Tool capability declarations remain trusted input
---

# Issue 70 post-merge closure

## 支持的结论

The repair was merged into framework `origin/main@a8a4d945` by PR #159 after all seven
remote CI jobs passed: dependency policy, Linux quality, Linux foundations/framework/tools/
learning tests, and Windows check. The local full framework gate also passed, including the
all-target/all-feature workspace tests and no-default-features check.

The independent rereview was PASS. The merged implementation rechecks Plan and readonly-Agent
admission at the effect boundary, preserves distinct denial reasons/sources, settles late policy
denial as blocked/Unavailable, and keeps terminal observation stages running. Issue #70 is
closed on GitHub, and the implementation worktree and source branch were deleted after merge.

## 来源与范围

This evidence closes the framework Finding only after repair, focused regressions, semantic
strict/change-evidence, full local gates, remote CI, independent rereview, PR merge, Issue
closure, and delivery worktree cleanup. It does not close any A2A Finding or SDK/CLI Finding.

## 已知缺口

The retry-delay race has no separate fixture, and third-party capability declarations remain
trusted input; both limits are already recorded in the repair verification evidence.
