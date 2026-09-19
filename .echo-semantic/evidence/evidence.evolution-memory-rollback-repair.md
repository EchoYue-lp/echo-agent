---
schema_version: 1
id: evidence.evolution-memory-rollback-repair
kind: evidence
observed_at: source:3a1ceacf4f7e1214698abf2ea09cc3217e426b87fadb0988c97926eb9dcd2bfa
source_refs:
  - src/evolution/mutation.rs
  - src/evolution/layer.rs
  - src/evolution/review.rs
  - src/evolution/mod.rs
  - docs/adr/0065-evolution-memory-audit-reconciliation.md
  - docs/en/25-self-improvement.md
  - docs/zh/25-self-improvement.md
  - echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs
  - CHANGELOG.md
supports: [finding.evolution-changelog-rollback-authority, behavior.eval-evolution]
limitations:
  - Finding #52 remains open because Skill/Rule/host rollback is not delivered by this memory slice
  - This repair covers canonical layered memory only; Skill/Rule rollback remains #54/#94/host owner
  - Raw Store readers still require manager reconciliation fencing
---

# Issue 52 canonical memory rollback repair candidate

## 支持的结论

`MemoryLayerManager` now resolves a typed `ChangeId` or `BatchId` to one
settled journal batch, expands any merge member to the complete batch, and
checks the latest journal generation for every affected key before mutation.
The inverse is a new durable batch with request ID and target lineage, and it
uses the existing Prepared -> projection -> idempotent audit -> Settled path.
Request retries return the original receipt, request-target conflicts fail
closed, and a rollback-of-rollback is a new batch. `ChangeLog` remains
append-only. Each inverse audit maps the original operation to its real reverse
type and records exact warm/hot before/after projection plus lineage without
changing the public `ChangeEntry` schema. Snapshot-only merge restoration is
retired in favor of the merge batch ID returned by `AppliedMemoryMerge`.

This is a candidate repair record only. It does not close the Finding or claim
Skill/Rule/host rollback semantics.

## 来源与范围

The candidate is limited to `MemoryLayerManager`, its private journal lineage,
the merge batch handle, public docs, and the demo51 contract.

## 已知缺口

The memory slice passed final advancing-base review and remains pending
remote-main delivery. The Finding remains open after that delivery because Skill/Rule/host
rollback is intentionally outside this candidate.
