---
schema_version: 1
id: asset.journal-delivery
kind: asset
title: EventJournal、Checkpoint 与 DeliveryLedger
asset_type: state_authority
status: active
risk: high
observed_at: source:bfa5b4590c617d8286f2b80571d5c450622d47c7425d6f1ecb1978bf85743352
boundary_refs: [boundary.observation-persistence-delivery]
code_refs: [echo-state/src/journal/mod.rs, echo-state/src/journal/file.rs, echo-state/src/journal/segmented.rs, echo-state/src/delivery.rs, docs/adr/0055-checkpoint-journal-identity.md]
consumer_refs: [src/state/mod.rs, echo-sdk-host/src/core_profile/persistence.rs]
behavior_refs: [behavior.observation-persistence]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation, evidence.checkpoint-journal-binding-repair, evidence.checkpoint-journal-binding-verification, evidence.checkpoint-journal-sdk-inventory]
finding_refs: [finding.checkpoint-journal-binding]
candidate_refs: []
---

# EventJournal、Checkpoint 与 DeliveryLedger

## 资产身份

Ordered event commit/replay、checkpointed reduction 与 typed delivery lifecycle 的持久 authority。

## 来源与消费者

Delivery、SDK persistence 和 framework consumers 使用 generic journal/checkpoint APIs。

## 生命周期

Allocate generation identity、prepare/append batch、reconcile unknown outcome、reduce/bound checkpoint/recover、claim/effect/ack/drain/settle/prune。

## 候选关系

Trace 和 transcript 不替代 journal fact。

## 未知与限制

每个应用 payload/retention policy 仍由消费者拥有。
