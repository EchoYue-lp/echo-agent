---
schema_version: 1
id: asset.subagent-runtime
kind: asset
title: Subagent Registry、Executor 与 Control
asset_type: state_authority
status: active
risk: high
observed_at: source:252362472c35fc62836123fbf064477b407af7bce21a21f16d231d594eebb136
boundary_refs: [boundary.task-subagent-workflow]
code_refs: [src/agent/subagent/registry.rs, src/agent/subagent/executor.rs, src/agent/subagent/control.rs, src/agent/subagent/events.rs, docs/adr/0033-subagent-factory-singleflight-publication.md]
consumer_refs: [src/tools/builtin/agent_dispatch.rs, src/agent/subagent/team/mod.rs]
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow, evidence.subagent-factory-singleflight-repair, evidence.subagent-factory-singleflight-verification]
finding_refs: [finding.task-subagent-attempt-link, finding.subagent-factory-cancellation, finding.subagent-factory-publication-race, finding.subagent-definition-catalog]
candidate_refs: []
---

# Subagent Registry、Executor 与 Control

## 资产身份

Subagent definitions/factory、所有 dispatch modes、attempt-scoped control/event/outcome 的 canonical runtime。

## 来源与消费者

Agent tool、Team runtime、Hook actions 和 programmatic callers 消费。

## 生命周期

Register/revision-scoped factory resolve、compile prompt/isolate、dispatch、message/interrupt/join、typed settle/replay。

## 候选关系

与 Task claim 必须关联但不替代 Task authority。

## 未知与限制

Team/SDK dispatch丢失TaskClaim到SubagentAttempt identity和definition catalog漂移仍为开放Finding；factory cancellation/publication已具备闭合证据。
