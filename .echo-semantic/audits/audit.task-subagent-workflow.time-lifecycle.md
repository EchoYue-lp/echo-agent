---
schema_version: 1
id: audit.task-subagent-workflow.time-lifecycle
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: time_lifecycle
freshness: examined
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.background-task-wait, finding.command-cell-retention-lease-prune-race, finding.command-cell-cancel-artifact-settlement]
challenges:
  background-wait-and-panic:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-orchestration/src/tasks/background_task.rs]
    evidence_refs: [evidence.task-subagent-workflow]
  command-cell-retention:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-core/src/tools/cell.rs, echo-orchestration/src/tasks/command_cell.rs, docs/adr/0025-deterministic-command-cell-watcher.md]
    evidence_refs: [evidence.task-subagent-workflow, evidence.effects-extensions]
  cancel-and-artifact-finalization:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-orchestration/src/tasks/command_cell.rs]
    evidence_refs: [evidence.task-subagent-workflow]
---

# BackgroundTask 与 CommandCell 时间生命周期审计

## 审查范围

审查 process-local BackgroundTask wait/cancel/panic/admission，以及 CommandCell watcher lease、prune、deadline、cancel、artifact finalize、terminal 和 shutdown。

## 已检查故障假设

验证 Notify lost wakeup、多观察者 result、panic terminal，CommandCell retention scan/acquire/remove 竞态，以及普通 cancel 是否受 bounded cleanup grace 约束。

## 实际实现路径与证据

BackgroundTask 在检查 result 后才注册不保存 permit 的 Notify waiter，存在 lost wakeup；首个 waiter take 结果后其它 waiter 无 status fallback；future panic 绕过 final status。CommandCell wait 的注册顺序正确，normal/deadline/shutdown terminal 在 drain/finalize 后发布；但 prune 在扫描 lease 后无条件 remove，可删除并发新 lease。普通 stop/owner cancel 后 artifact finalization 只监听 manager shutdown 和原始 deadline，可能远超声明的 5 秒 grace。

## 问题记录

确认 BackgroundTask Finding；新增 CommandCell retention lease prune race 与 cancel/artifact settlement。TaskSpawner admission 不监听 cancel/deadline且允许零并发，纳入 BackgroundTask repair 范围。

## 残余风险

CommandCell supervisor panic containment、manager Drop best-effort close 和跨平台 process-group cleanup 仍需下一 risk/repair 复核。

## 未检查项

未执行 loom/stress、文件系统故障注入或跨平台进程测试；没有验证普通 cancel + blocking finalizer 的持久测试。
