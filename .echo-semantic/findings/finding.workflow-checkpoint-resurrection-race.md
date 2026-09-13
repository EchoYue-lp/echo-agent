---
schema_version: 1
id: finding.workflow-checkpoint-resurrection-race
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: failure_concurrency
focus: [data_durability, time_lifecycle, state_authority]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: []
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: [audit.task-subagent-workflow.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Workflow tag 可复活已领取 checkpoint

## 问题

`tag_checkpoint` 以 load-modify-save 实现，能与 resume claim 交错：tag 先读、resume 消费、tag 后保存会重新发布同一 checkpoint。

## 触发条件与影响

并发 tag/resume 时，已经执行或正在执行的 continuation 可再次被发现和领取，造成重复节点 effect。

## 证据

`echo-orchestration/src/workflow/graph.rs` 与 CheckpointStore 单方法原子合同展示缺少跨 load/claim/save generation CAS。

## 处理记录

Failure-concurrency Audit 确认；后续 repair 需 CAS/lease identity 与确定性交错测试。
