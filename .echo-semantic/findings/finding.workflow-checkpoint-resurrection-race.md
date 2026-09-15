---
schema_version: 1
id: finding.workflow-checkpoint-resurrection-race
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [data_durability, time_lifecycle, state_authority]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: []
evidence_refs: [evidence.task-subagent-workflow, evidence.workflow-checkpoint-claim-settlement-repair, evidence.workflow-checkpoint-claim-settlement-verification]
audit_refs: [audit.task-subagent-workflow.failure-concurrency, audit.workflow-checkpoint-claim-settlement-rereview]
decision_refs: []
repair_evidence_refs: [evidence.workflow-checkpoint-claim-settlement-repair]
verification_evidence_refs: [evidence.workflow-checkpoint-claim-settlement-verification]
rereview_audit_refs: [audit.workflow-checkpoint-claim-settlement-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Workflow tag 可复活已领取 checkpoint

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/110

## 问题

`tag_checkpoint` 以 load-modify-save 实现，能与 resume claim 交错：tag 先读、resume 消费、tag 后保存会重新发布同一 checkpoint。

## 触发条件与影响

并发 tag/resume 时，已经执行或正在执行的 continuation 可再次被发现和领取，造成重复节点 effect。

## 证据

`echo-orchestration/src/workflow/graph.rs` 与 CheckpointStore 单方法原子合同展示缺少跨 load/claim/save generation CAS。

## 处理记录

Tag只对pending checkpoint执行generation CAS；active或stale claim使用attempt与file lock隔离，
不能被load-modify-save复活。跨实例交错、crash cut和远程E2E通过独立复审。
