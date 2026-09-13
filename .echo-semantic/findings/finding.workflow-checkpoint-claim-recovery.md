---
schema_version: 1
id: finding.workflow-checkpoint-claim-recovery
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: data_durability
focus: [failure_concurrency, time_lifecycle, state_authority]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: [audit.task-subagent-workflow.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Workflow checkpoint claim 缺少失败恢复

## 问题

File checkpoint store 通过 rename 领取后读取并删除 `.claim`；读取/解析/删除失败或进程崩溃会留下普通 list/load 不再发现的 claim，resume 又在执行下一节点前消费 checkpoint。

## 触发条件与影响

Claim 后任一失败可能永久隐藏可恢复 continuation；state snapshot 失败还可能被空值替代。

## 证据

`echo-orchestration/src/workflow/checkpoint_store.rs` 与 `workflow/graph.rs` 的 claim/resume 顺序提供源码证据。

## 处理记录

Discovery 记录；下一阶段以 crash-cut 测试决定 requeue、保留 claim 或显式失败记录合同。
