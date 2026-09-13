---
schema_version: 1
id: finding.task-patch-claim-race
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: failure_concurrency
focus: [state_authority, data_durability, time_lifecycle]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Task relation patch 可能覆盖 live claim

## 问题

Relation patch 只以 graph revision 做 CAS；runtime claim 不递增该 revision，且 SetStatus/Skip 对 execution drift 有豁免，旧 patch snapshot 可能覆盖并发创建的 live claim。

## 触发条件与影响

Patch 先读取 Pending revision，runtime 随后 claim，同一 patch 再提交时可能清除 physical ownership，导致执行中的 attempt 与持久 Task 状态分离。

## 证据

`echo-orchestration/src/tasks/revisioned.rs` 的 compare/commit 与 patch effects 提供静态反例；现有测试未覆盖 load -> claim -> commit 精确交错。

## 处理记录

Discovery 记录为高风险 authority conflict；下一阶段用确定性交错测试审计后再决定 revision 或 CAS 修复。
