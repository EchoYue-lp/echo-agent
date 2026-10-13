---
schema_version: 1
id: asset.task-graph-authority
kind: asset
title: Revisioned Task Graph Authority
asset_type: state_authority
status: active
risk: high
observed_at: source:34d5f5574c9cf5a645970cd8b58367c39bce268630488b68d3ea54e996728e17
boundary_refs:
- boundary.task-subagent-workflow
code_refs:
- echo-orchestration/src/tasks/revisioned.rs
- echo-orchestration/src/tasks/runtime.rs
- echo-orchestration/src/tasks/runtime_service.rs
- echo-orchestration/src/tasks/runtime_executor.rs
- docs/adr/0060-task-execution-store-cas-and-local-interruption.md
consumer_refs:
- src/tasks.rs
- src/agent/subagent/team/mod.rs
- tests/facade_smoke.rs
behavior_refs:
- behavior.task-subagent-execution
rule_refs:
- rule.task-subagent-authority
evidence_refs:
- evidence.task-subagent-workflow
- evidence.task-runtime-store-control-repair
- evidence.task-runtime-store-control-verification
finding_refs:
- finding.task-patch-claim-race
- finding.task-subagent-attempt-link
- finding.workflow-dag-authority
- finding.task-runtime-store-control
candidate_refs:
- asset.workflow-runtime
---


# Revisioned Task Graph Authority

## 资产身份

Task spec/execution、revision、claim、ready frontier、retry/cancel/pause/settlement 的 canonical authority。

## 来源与消费者

Task tools、Team runtime、SDK task facade 与应用 controller 消费。

## 生命周期

相同 relation revision 上的完整 execution CAS 防止并发 relation patch、claim、结果、中断请求相互覆盖；结果与 Completed 同事务。支持声明先于任何执行副作用；重建 service 后读取同图结果，成功节点不重跑。

Create/patch/load、claim attempt、dispatch、resolve/requeue/pause/cancel、reload safe point。

## 候选关系

与 generic Workflow graph 相邻，是否存在重复需 audit；不得先合并。

## 未知与限制

新修复证明通用 framework 机制与 InMemory/代理 backend 主路径。第三方 backend 的真正持久事务、跨进程 owner fencing、SDK wire 与 CLI consumer 编译均有独立证据义务，本框架全量门禁不代替它们。

Relation revision 与 concurrent claim patch 的交错已形成 Finding。
