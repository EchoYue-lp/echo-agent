---
schema_version: 1
id: audit.task-subagent-workflow.data-durability
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: data_durability
freshness: examined
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.scheduler-cache-delivery, finding.scheduler-task-id-uniqueness, finding.scheduler-control-fire-race]
challenges:
  scheduler-store-cache:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-orchestration/src/scheduler/runner.rs, echo-orchestration/src/scheduler/cron_task.rs]
    evidence_refs: [evidence.task-subagent-workflow]
  callback-delivery-cut:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-orchestration/src/scheduler/runner.rs]
    evidence_refs: [evidence.task-subagent-workflow]
  definition-identity-and-migration:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-orchestration/src/scheduler/cron_task.rs, echo-orchestration/src/scheduler/runner.rs]
    evidence_refs: [evidence.task-subagent-workflow]
---

# Scheduler Store、Cache 与 Delivery 数据持久性审计

## 审查范围

审查 CronTaskStore、runner cache、tick/run_once、callback effect、last-run receipt、legacy migration、task identity 与控制并发。

## 已检查故障假设

验证 store/cache 是否线性一致、effect 与 receipt crash cut 是否有持久 claim、migration 是否覆盖目标数据、重复 ID 与 disable/remove 是否仍影响错误 occurrence。

## 实际实现路径与证据

Fire/run_once 只更新 Store，list/tick 读取旧 cache；add/remove/status 是 store 后 cache 的双写，取消可中断两者之间。Occurrence 只用进程内 last_fired，callback effect 与 update_last_run 没有持久 identity/claim/ledger。Legacy migration 无条件覆盖目标并忽略 legacy 删除失败。重复 ID 会共享 last_fired、只更新首项或被批量删除；tick 克隆待执行项后释放锁，随后成功 disable/remove 仍不能撤销已捕获 callback。

## 问题记录

确认 `finding.scheduler-cache-delivery`，新增 task ID uniqueness 与 control/fire race。Cache 一致性、ID 校验、migration collision 不依赖 delivery policy 裁决即可修复；跨 crash guarantee 进入 semantic-decide。

## 残余风险

源码只支持进程存活期间 best-effort trigger，不能证明 at-most-once、at-least-once 或 exactly-once；任意 callback effect 也不能由 Scheduler 单独实现 exactly-once。

## 未检查项

Focused scheduler tests 7 passed，但未覆盖 cache freshness、双写交错、migration collision、持久化故障、crash cut、重复 ID 或控制/fire race；未检查多进程 writer。
