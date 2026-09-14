---
schema_version: 1
id: finding.background-task-wait
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, state_authority]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow, evidence.background-task-terminal-authority-repair, evidence.background-task-terminal-authority-verification]
audit_refs: [audit.task-subagent-workflow.time-lifecycle, audit.background-task-terminal-authority-rereview]
decision_refs: []
repair_evidence_refs: [evidence.background-task-terminal-authority-repair]
verification_evidence_refs: [evidence.background-task-terminal-authority-verification]
rereview_audit_refs: [audit.background-task-terminal-authority-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# BackgroundTask wait 与多观察者生命周期缺口

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/39

## 问题

Result 检查与 Notify waiter 注册之间存在 lost-wakeup 窗口；首个 waiter take 结果后，后续 waiter 没有实现注释承诺的 status-based reporting，真实 panic 也可能不结算状态。

## 触发条件与影响

任务恰在检查与订阅间完成、多 waiter 同时等待或 future panic 时，wait 可能超时/挂起或停留 Running。

## 证据

`echo-orchestration/src/tasks/background_task.rs` 的 wait、notify、result take 和 panic classification 提供源码反例。

## 处理记录

Issue #39追踪。BackgroundTask已用单一共享state原子发布status/result/typed panic provenance，Clone handle的多waiter不会挂起；TaskSpawner admission/execution共享cancel与absolute deadline，child execution task由JoinHandle监督，zero config与panic均归约terminal。Repair、verification与独立复审已闭合本Finding。GitHub Issue保持open，等待远程main交付。
