---
schema_version: 1
id: finding.subagent-factory-cancellation
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, state_authority, contract_evidence]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: [audit.task-subagent-workflow.state-authority]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Subagent lazy factory 取消后无法恢复 publication

## 问题

`SubagentRegistry::get_agent` 在等待 factory create 前把名称加入 `instantiating`，清理只发生在 await 正常返回后；dispatch future 被取消或 abort 时名称可永久残留。

## 触发条件与影响

Factory create 期间发生 caller cancel、runtime grace timeout 或 task abort 后，后续相同名称 resolve 只能反复等待并超时，Subagent capability 无法自行恢复。

## 证据

`src/agent/subagent/registry.rs` 展示 instantiating publication；`echo-orchestration/src/tasks/runtime_executor.rs` 展示超时 abort 路径。

## 处理记录

Discovery 记录；后续 lifecycle audit 应验证取消安全的 publication guard、waiter 唤醒和 stale generation 防护。
