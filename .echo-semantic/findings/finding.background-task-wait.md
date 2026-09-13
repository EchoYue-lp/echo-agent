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
audit_refs: [audit.task-subagent-workflow.time-lifecycle]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
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

Time-lifecycle Audit 确认；TaskSpawner admission 不监听 cancel/deadline且允许零并发也纳入本 Finding repair 范围，后续用确定性调度测试修复单一 terminal authority。
