---
schema_version: 1
id: evidence.evolution-memory-rollback-verification
kind: evidence
observed_at: source:87b717676a7b51e213630677989947777b4bed441acd8c7d655fb6c96dca77ad
source_refs:
  - src/evolution/layer.rs
  - src/evolution/mutation.rs
  - src/evolution/review.rs
  - echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs
supports: [finding.evolution-changelog-rollback-authority, behavior.eval-evolution]
limitations:
  - Remote-main delivery is pending
  - Crash tests simulate restart and retained journal facts, not physical power loss
---

# Issue 52 focused verification candidate

## 支持的结论

Focused rollback tests cover settled single-key restore, merge-group restore,
tip and ABA rejection, request-id idempotency and conflict, rollback of a
rollback, restart receipt recovery, and legacy batch decoding without lineage.
The inverse matrix verifies Create/Delete/Update/Promote/Demote types, layer
direction, metadata-only restore, multiline roundtrip, and machine-readable
projection/lineage summaries. The demo51 contract covers the public preview
and receipt path. After merging `origin/main@577e0b8c`, the lane passed 171
evolution tests, 29 memory tests, 13 example contracts, both required Clippy
passes, the all-target/all-feature workspace test suite, no-default workspace
check, all 16 independent root feature checks, formatter check, and strict
semantic snapshot verification. The first full test attempt exhausted local
disk during linking; after clearing derived Cargo caches and disabling
incremental compilation, the complete command passed without test failures.

## 来源与范围

The focused and complete gates run against the advancing-base issue-52
worktree and do not claim remote-main delivery.

## 已知缺口

Final advancing-base independent review passed with no Critical, Important, or
Minor findings. Remote-main delivery remains pending. Skill/Rule/host rollback
remains outside this memory-only slice.
