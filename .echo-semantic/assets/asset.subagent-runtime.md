---
schema_version: 1
id: asset.subagent-runtime
kind: asset
title: Subagent Registry、Executor 与 Control
asset_type: state_authority
status: active
risk: high
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
boundary_refs: [boundary.task-subagent-workflow]
code_refs: [src/agent/subagent/registry.rs, src/agent/subagent/executor.rs, src/agent/subagent/control.rs, src/agent/subagent/events.rs]
consumer_refs: [src/tools/builtin/agent_dispatch.rs, src/agent/subagent/team/mod.rs]
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
finding_refs: [finding.task-subagent-attempt-link, finding.subagent-factory-cancellation, finding.subagent-definition-catalog]
candidate_refs: []
---

# Subagent Registry、Executor 与 Control

## 资产身份

Subagent definitions/factory、所有 dispatch modes、attempt-scoped control/event/outcome 的 canonical runtime。

## 来源与消费者

Agent tool、Team runtime、Hook actions 和 programmatic callers 消费。

## 生命周期

Register/resolve、compile prompt/isolate、dispatch、message/interrupt/join、typed settle/replay。

## 候选关系

与 Task claim 必须关联但不替代 Task authority。

## 未知与限制

Team/SDK dispatch 丢失 TaskClaim 到 SubagentAttempt identity、lazy factory cancellation 和 definition catalog 漂移已形成 Findings。
