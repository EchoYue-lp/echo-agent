---
schema_version: 1
id: evidence.scheduler-occurrence-authority-verification
kind: evidence
observed_at: 8c8d8aa469233732276df8d313495a5dce67961d
source_refs:
  - echo-orchestration/src/scheduler/cron_task.rs
  - echo-orchestration/src/scheduler/runner.rs
  - docs/adr/0042-scheduler-occurrence-authority.md
supports: [behavior.task-subagent-execution]
limitations:
  - 不证明进程崩溃后的occurrence replay或callback delivery guarantee
  - 不证明跨进程CronTaskStore writer线性一致
---

# Scheduler occurrence authority验证证据

## 支持的结论

CronTaskStore和SchedulerRunner定向测试共15项通过，覆盖重复/空ID拒绝、legacy migration
目标冲突、store mutation后cache刷新、Disabled手动触发、control与admission竞态、旧definition
settlement以及remove/re-add同ID stale occurrence fencing。`echo_orchestration` all-target/
all-feature 344项、Clippy warnings门禁和formatter同时通过。

## 来源与范围

独立reviewer沿control lock、epoch、definition identity和store validation检查实现，确认#85
与#86可关闭，同时明确#84必须保持open。审查排除callback crash replay和跨进程Store。

## 已知缺口

完整workspace与远端CI在汇总MR前执行；durable occurrence投递由#84继续追踪。
