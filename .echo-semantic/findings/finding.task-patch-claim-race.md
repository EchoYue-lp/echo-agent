---
schema_version: 1
id: finding.task-patch-claim-race
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [state_authority, data_durability, time_lifecycle]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow, evidence.task-patch-claim-cas-repair, evidence.task-patch-claim-cas-verification]
audit_refs: [audit.task-subagent-workflow.state-authority, audit.task-patch-claim-cas-rereview]
decision_refs: []
repair_evidence_refs: [evidence.task-patch-claim-cas-repair]
verification_evidence_refs: [evidence.task-patch-claim-cas-verification]
rereview_audit_refs: [audit.task-patch-claim-cas-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Task relation patch 可能覆盖 live claim

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/27

## 问题

Relation patch 只以 graph revision 做 CAS；runtime claim 不递增该 revision，且 SetStatus/Skip 对 execution drift 有豁免，旧 patch snapshot 可能覆盖并发创建的 live claim。

## 触发条件与影响

Patch 先读取 Pending revision，runtime 随后 claim，同一 patch 再提交时可能清除 physical ownership，导致执行中的 attempt 与持久 Task 状态分离。

## 证据

基准实现的 compare/commit 与 patch effects 提供静态反例，确定性 red 测试确认 `load -> claim -> stale commit` 可覆盖 live claim。当前 canonical producer、exact execution CAS 和 interleaving regression test 共同覆盖该路径。

## 处理记录

`TaskRevisionService` 现携带读取时完整 execution snapshot；Store 对 claim、retry、settlement 与 patch 做 typed conflict。SDK 合同同步并通过独立复审，本 Finding 已关闭；第三方 Store 原子性保留为复用方验证责任。
