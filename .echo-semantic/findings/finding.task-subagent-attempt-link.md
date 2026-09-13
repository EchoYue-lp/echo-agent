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
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# TaskClaim 与 SubagentAttempt identity 未闭合

## 问题

Team 与 SDK runtime controller 接收 TaskClaim 后调用普通 Subagent dispatch，实际 Subagent execution 使用随机 ID，task/attempt/plan revision lineage 为空。

## 触发条件与影响

TaskRuntime 执行 Team 或 SDK task 时，Task CAS claim 与 Subagent control/event identity 不能稳定关联，影响精确 interrupt、command、replay 和恢复。

## 证据

`src/agent/subagent/team/mod.rs`、`echo-sdk-host/src/core_profile/facade/task_runtime.rs` 与 `src/agent/subagent/executor.rs` 显示 claim 被丢弃和 lineage 缺失。

## 处理记录

Discovery 记录；下一阶段审计 typed claim-to-attempt 传递，不新增第二 identity authority。
