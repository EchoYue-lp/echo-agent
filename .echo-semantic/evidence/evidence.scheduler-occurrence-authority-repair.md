---
schema_version: 1
id: evidence.scheduler-occurrence-authority-repair
kind: evidence
observed_at: 729a80f2748808197b8578f7770bd361fd084d60
source_refs:
  - echo-orchestration/src/scheduler/cron_task.rs
  - echo-orchestration/src/scheduler/runner.rs
  - docs/adr/0042-scheduler-occurrence-authority.md
supports:
  - behavior.task-subagent-execution
  - rule.task-subagent-authority
limitations:
  - 不定义进程崩溃后的 occurrence replay、at-most-once 或 at-least-once delivery
  - 不提供跨进程 writer 的 compare-and-swap；CronTaskStore 仍是单进程共享锁模型
---

# Scheduler occurrence authority 修复证据

## 支持的结论

`CronTaskStore` 在 load、add、save 和 legacy migration 入口校验非空且唯一的
CronTask ID；重复定义 fail closed，Store backend 已存在的目标值不会被 legacy
文件覆盖。SchedulerRunner 在每次成功 store mutation 后重新加载完整快照，使
`tasks` 只作为 derived cache，不再保留旧的 `last_run` projection。

Tick 保存 `task.id + created_at + scheduled_at` occurrence identity 和每个 task 的
control epoch。callback 创建前在 runner control lock 下重新确认同一任务定义仍
存在、为 Enabled 且 epoch 未被 control mutation 递增；在该 admission 点之后，
disable/remove 不撤回已接纳 invocation。callback settlement
用 captured definition identity 更新 last-run，旧定义不能写入被删除后重建的同 ID
任务。`run_once` 仍是显式手动触发路径，但不会绕过 Disabled 状态，并在持久化
成功后刷新 cache；控制/admission gate 只约束 tick 捕获的 scheduled occurrence。

## 验证

- `cargo test -p echo_orchestration scheduler::cron_task::tests`：7 passed
- `cargo test -p echo_orchestration scheduler::runner::tests`：8 passed
- `cargo clippy -p echo_orchestration --all-targets --all-features --locked -- -D warnings`：passed
- `cargo fmt --all -- --check`：passed

## 来源与范围

本 Evidence 绑定 Git commit `812fca0261039d7dd686b2b5edf1a00465b54275`，只覆盖
SchedulerRunner/CronTaskStore 的本地 authority、cache freshness、definition
identity 与 control/admission 顺序；不把 callback effect 本身伪装成可回滚事务。

## 已知缺口

未引入 durable occurrence claim 或 callback delivery ledger，因此进程崩溃发生在
callback effect 与 `last_run` 持久化之间时，恢复后的 replay/loss 行为仍待产品决策。
迁移保护只覆盖同一 Store backend 的目标 key 已存在场景；跨进程同时创建两个
backend 的文件迁移仍需要更高层的文件 lease 或单独迁移协议。
