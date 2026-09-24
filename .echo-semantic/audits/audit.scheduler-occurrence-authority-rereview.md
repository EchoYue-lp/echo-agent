---
schema_version: 1
id: audit.scheduler-occurrence-authority-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: failure_concurrency
freshness: examined
revision: source:d0d70f0819595200d148564ce11cb55de583ba943d7a5554b4c06a0f6dcb8af9
finding_refs: [finding.scheduler-control-fire-race, finding.scheduler-task-id-uniqueness, finding.scheduler-cache-delivery]
challenges:
  control-admission-linearization:
    revision: source:d0d70f0819595200d148564ce11cb55de583ba943d7a5554b4c06a0f6dcb8af9
    source_refs: [echo-orchestration/src/scheduler/runner.rs]
    evidence_refs: [evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
  unique-definition-identity:
    revision: source:d0d70f0819595200d148564ce11cb55de583ba943d7a5554b4c06a0f6dcb8af9
    source_refs: [echo-orchestration/src/scheduler/cron_task.rs, echo-orchestration/src/scheduler/runner.rs]
    evidence_refs: [evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
  durable-occurrence-replay:
    revision: source:d0d70f0819595200d148564ce11cb55de583ba943d7a5554b4c06a0f6dcb8af9
    source_refs: [echo-orchestration/src/scheduler/runner.rs, docs/adr/0042-scheduler-occurrence-authority.md]
    evidence_refs: [evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
  public-store-cache-bypass:
    revision: source:d0d70f0819595200d148564ce11cb55de583ba943d7a5554b4c06a0f6dcb8af9
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
均已进入 main。原 `main@e15cc17f` 反例显示：caller 保留 `CronTaskStore` clone 后直接 disable/remove/update，
runner cache 不会自动刷新，`list_tasks`、`tick` 与 `run_once` 仍可观察旧 definition。

当前任务分支复审沿公开写入、runner 构造、取消、同路径 file handle、同一 backend instance
的 reanchored/unanchored handle，以及 legacy source migration 路径核对写入许可。
Runner 在同一 mutation lock 下先读取持久快照再发布 owner；公开 mutation 必须在
该锁内检查 owner，runner 管理与 callback settlement 持私有 owner 更新，并把成功提交的
快照投影到 cache。已提交但 caller 随后取消的写入由 runner claim 等锁后读取。
目标 backend 已有定义时 migration 不触碰 legacy source；需要迁移时检查 live source owner。

## 问题记录

#85 与 #86 无剩余 blocker。`main@e15cc17f` 的 #84 Important gap 是 public store mutation
可绕过 runner cache；当前任务分支在上述单进程共享 owner 边界内闭合。独立只读 reviewer
对最终 diff、ADR 0042、源码、文档和 58/58 focused tests 事实给出 pass、0 action items；
reviewer 未自行运行测试。本地完整门禁已通过，此结论仍待 PR/CI 与远端 main 验证。

## 残余风险

callback effect 与 terminal receipt 之间的 crash window 采用 at-least-once，并要求 callback 按
`occurrence_id` 幂等。Cron offline misfire、跨进程 Store writer、不同 backend 对象指向同一
物理存储及直接 backend 编辑仍在单进程 owner 合同外。

## 未检查项

未执行跨进程 Store 并发或真实进程 kill；独立 reviewer 未自行重跑测试。Issue #84
仍待远端 main 交付，不等待 SDK、CLI、website 或其它 consumer。
