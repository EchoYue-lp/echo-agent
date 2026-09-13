---
schema_version: 1
id: finding.background-task-wait
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, state_authority]
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

# BackgroundTask wait 与多观察者生命周期缺口

## 问题

Result 检查与 Notify waiter 注册之间存在 lost-wakeup 窗口；首个 waiter take 结果后，后续 waiter 没有实现注释承诺的 status-based reporting，真实 panic 也可能不结算状态。

## 触发条件与影响

任务恰在检查与订阅间完成、多 waiter 同时等待或 future panic 时，wait 可能超时/挂起或停留 Running。

## 证据

`echo-orchestration/src/tasks/background_task.rs` 的 wait、notify、result take 和 panic classification 提供源码反例。

## 处理记录

Discovery 记录；下一阶段以确定性调度测试复核并修复单一 terminal authority。
