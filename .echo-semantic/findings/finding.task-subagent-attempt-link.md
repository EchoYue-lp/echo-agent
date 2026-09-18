---
schema_version: 1
id: finding.task-subagent-attempt-link
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [failure_concurrency, time_lifecycle, contract_evidence]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: [audit.task-subagent-workflow.state-authority]
decision_refs: []
repair_evidence_refs: [evidence.task-subagent-attempt-link-repair]
verification_evidence_refs: [evidence.task-subagent-attempt-link-verification]
rereview_audit_refs: [audit.task-subagent-attempt-link-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# TaskClaim 与 SubagentAttempt identity 未闭合

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/99

## 问题

Team 与 SDK runtime controller 接收 TaskClaim 后调用普通 Subagent dispatch，实际 Subagent execution 使用随机 ID，task/attempt/plan revision lineage 为空。

## 触发条件与影响

TaskRuntime 执行 Team 或 SDK task 时，Task CAS claim 与 Subagent control/event identity 不能稳定关联，影响精确 interrupt、command、replay 和恢复。

## 证据

`src/agent/subagent/team/mod.rs`、`echo-sdk-host/src/core_profile/facade/task_runtime.rs` 与 `src/agent/subagent/executor.rs` 显示 claim 被丢弃和 lineage 缺失。

## 处理记录

Framework 阶段已在 `f310825c418932cde02f661f0686f83f216771d6` 修复：TaskClaim 派生
唯一 physical attempt identity，Team/runtime dispatch、精确 interrupt、targeted abort、
join-to-CAS observation 与 recovery reconciliation 共享同一权威链。live registry 继续是
进程内投影，不成为第二持久状态机。

Finding 保持 open：独立 `echo-agent-sdk` Host adapter、durable command replay、framework pin、
inventory 与端到端合同仍未交付。Issue #99 只能在该第二阶段完成并重新验证后关闭。
