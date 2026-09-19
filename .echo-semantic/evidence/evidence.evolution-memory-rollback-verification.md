---
schema_version: 1
id: evidence.evolution-memory-rollback-verification
kind: evidence
observed_at: source:5f1004796277c714cad2bf5280fc86520c6459b24330dd6a931a52a59e72f971
source_refs:
  - src/evolution/layer.rs
  - src/evolution/mutation.rs
  - src/evolution/review.rs
  - echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs
supports: [finding.evolution-changelog-rollback-authority, behavior.eval-evolution]
limitations:
  - Final workspace gates, independent review, semantic strict snapshot and remote-main delivery are pending
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
and receipt path. Final command
receipts must be appended after the last implementation change; this evidence
therefore remains a candidate until the lane's focused evolution/memory/example
commands, formatter, Clippy and semantic verification pass.

## 来源与范围

The focused checks run against the issue-52 worktree and do not claim remote
main delivery.

## 已知缺口

Independent review, complete workspace gates, semantic strict snapshot, and
remote-main delivery remain pending.
