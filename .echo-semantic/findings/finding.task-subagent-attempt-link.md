---
schema_version: 1
id: finding.task-subagent-attempt-link
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: state_authority
focus: [failure_concurrency, time_lifecycle, contract_evidence]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow, evidence.task-subagent-attempt-link-repair, evidence.task-subagent-attempt-link-verification, evidence.task-subagent-external-control-handle-repair, evidence.task-subagent-external-control-handle-verification, evidence.framework-only-finding-closure-verification]
audit_refs: [audit.task-subagent-workflow.state-authority, audit.task-subagent-attempt-link-rereview, audit.task-subagent-external-control-handle-rereview]
decision_refs: [decision-adr-0058-task-claim-subagent-attempt-control]
repair_evidence_refs: [evidence.task-subagent-attempt-link-repair, evidence.task-subagent-external-control-handle-repair]
verification_evidence_refs: [evidence.task-subagent-attempt-link-verification, evidence.task-subagent-external-control-handle-verification, evidence.framework-only-finding-closure-verification]
rereview_audit_refs: [audit.task-subagent-attempt-link-rereview, audit.task-subagent-external-control-handle-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# TaskClaim 与 SubagentAttempt identity 链路

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/99

## 问题

Team 与外部 runtime controller 曾在接收 TaskClaim 后调用普通 Subagent dispatch，实际
Subagent execution 使用随机 ID，task/attempt/plan revision lineage 为空。

## 触发条件与影响

TaskRuntime 执行 Team 或 adapter task 时，Task CAS claim 与 Subagent control/event identity
不能稳定关联，影响精确 interrupt、command、replay 和恢复。

## 证据

`src/agent/subagent/team/mod.rs`、`echo-orchestration/src/tasks/runtime_executor.rs` 与
`src/agent/subagent/executor.rs` 覆盖 claim、dispatch 和 lineage 边界。

## 处理记录

Framework 已修复：TaskClaim 派生
唯一 physical attempt identity，Team/runtime dispatch、精确 interrupt、targeted abort、
join-to-CAS observation 与 recovery reconciliation 共享同一权威链。live registry 继续是
进程内投影，不成为第二持久状态机。

PR #133 与 #134 进一步加入 scope-bound `SubagentAttemptControlHandle`，让 Team 与外部
adapter 共享 reservation、dispatch、interrupt projection、retire 与 reconcile 实现，并已进入
framework main。该切片不新增 durable authority；Host command journal/replay、pin 与 wire mapping
是 consumer-owned outcome，不是本 Finding 或 Issue #99 的关闭条件。
