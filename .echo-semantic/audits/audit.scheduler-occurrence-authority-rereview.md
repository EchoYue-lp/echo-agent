---
schema_version: 1
id: audit.scheduler-occurrence-authority-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: failure_concurrency
freshness: examined
revision: source:3d3fb558349d604e3762588a5db974417956f949c929c8d8cae30ab94558379b
finding_refs: [finding.scheduler-control-fire-race, finding.scheduler-task-id-uniqueness, finding.scheduler-cache-delivery]
challenges:
  control-admission-linearization:
    revision: source:3d3fb558349d604e3762588a5db974417956f949c929c8d8cae30ab94558379b
    source_refs: [echo-orchestration/src/scheduler/runner.rs]
    evidence_refs: [evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
  unique-definition-identity:
    revision: source:3d3fb558349d604e3762588a5db974417956f949c929c8d8cae30ab94558379b
    source_refs: [echo-orchestration/src/scheduler/cron_task.rs, echo-orchestration/src/scheduler/runner.rs]
    evidence_refs: [evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
  durable-occurrence-replay:
    revision: source:3d3fb558349d604e3762588a5db974417956f949c929c8d8cae30ab94558379b
    source_refs: [echo-orchestration/src/scheduler/runner.rs, docs/adr/0042-scheduler-occurrence-authority.md]
    evidence_refs: [evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
  public-store-cache-bypass:
    revision: source:3d3fb558349d604e3762588a5db974417956f949c929c8d8cae30ab94558379b
    source_refs: [echo-orchestration/src/scheduler/cron_task.rs, echo-orchestration/src/scheduler/runner.rs]
    evidence_refs: [evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
---

# Scheduler occurrence authority独立复审

## 审查范围

独立 reviewer 检查 CronTask ID、public store/cache 刷新、control/admission 线性化、definition
identity、remove/re-add fencing、durable occurrence replay 和 callback settlement。

## 已检查故障假设

检查重复ID导致控制作用于不同定义、disable/remove成功后旧捕获callback仍被接纳、旧callback
回写新同ID任务、migration覆盖目标Store，以及callback effect与last_run之间崩溃无法判定。

## 实际实现路径与证据

唯一 ID、control epoch、definition identity、control-lock admission 与 DeliveryLedger occurrence
均已进入 main。当前反例显示：caller 保留 `CronTaskStore` clone 后直接 disable/remove/update，
runner cache 不会自动刷新，`list_tasks`、`tick` 与 `run_once` 仍可观察旧 definition。

## 问题记录

#85 与 #86 无剩余 blocker；#84 有一个 Important framework gap：public store mutation 可以绕过
runner mutation owner 与 cache synchronization。

## 残余风险

callback effect 与 terminal receipt 之间的 crash window 采用 at-least-once，并要求 callback 按
`occurrence_id` 幂等。Cron offline misfire 和跨进程 Store writer 仍是明确非目标。

## 未检查项

未执行跨进程 Store 并发或真实进程 kill。当前 review 只将 #84 保持 open 于 public
store/cache framework gap，不等待 SDK、CLI、website 或其它 consumer。
