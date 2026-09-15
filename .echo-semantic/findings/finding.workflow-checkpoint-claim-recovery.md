---
schema_version: 1
id: finding.workflow-checkpoint-claim-recovery
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: data_durability
focus: [failure_concurrency, time_lifecycle, state_authority]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow, evidence.workflow-checkpoint-claim-settlement-repair, evidence.workflow-checkpoint-claim-settlement-verification]
audit_refs: [audit.task-subagent-workflow.failure-concurrency, audit.workflow-checkpoint-claim-settlement-rereview]
decision_refs: []
repair_evidence_refs: [evidence.workflow-checkpoint-claim-settlement-repair]
verification_evidence_refs: [evidence.workflow-checkpoint-claim-settlement-verification]
rereview_audit_refs: [audit.workflow-checkpoint-claim-settlement-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Workflow checkpoint claim 缺少失败恢复

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/109

## 问题

File checkpoint store 通过 rename 领取后读取并删除 `.claim`；读取/解析/删除失败或进程崩溃会留下普通 list/load 不再发现的 claim，resume 又在执行下一节点前消费 checkpoint。

## 触发条件与影响

Claim 后任一失败可能永久隐藏可恢复 continuation；state snapshot 失败还可能被空值替代。

## 证据

`echo-orchestration/src/workflow/checkpoint_store.rs` 与 `workflow/graph.rs` 的 claim/resume 顺序提供源码证据。

## 处理记录

commit `eb8744566dcd5a734531869ebde9f3506b132163`实现attempt-fenced renewable claim、
失败requeue、成功ack、crash-cut恢复与四语言远程结算；最终独立复审通过。
